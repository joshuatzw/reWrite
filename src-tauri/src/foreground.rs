//! Foreground-window inspection to choose the rewrite output format.
//!
//! The app the user is about to paste into decides whether we emit rich HTML
//! (bold, bullet lists) or plain text. Rich-text targets — Outlook, Word,
//! Gmail/Outlook in a browser — get HTML; everything else, and anything we
//! cannot positively identify, falls back to plain text. Detection MUST happen
//! while the target app is still frontmost, i.e. before we show the overlay or
//! processing window (both steal foreground).

/// The output format to request from the model and paste back.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum OutputFormat {
    /// Rich inline HTML — for composers that render it (Outlook, Word, Gmail…).
    Html,
    /// Plain text — the safe default for anything we don't recognise.
    #[default]
    PlainText,
}

/// Detect the frontmost app and decide the output format for it. Any failure
/// to inspect the foreground window resolves to `PlainText`.
///
/// Takes an `AppHandle` because the macOS backend needs to marshal the
/// AppKit lookup onto the main thread (see `detect_impl` below); the
/// Windows/other-platform backends ignore it.
pub fn detect(app: &tauri::AppHandle) -> OutputFormat {
    detect_impl(app)
}

/// Decide the format from a lowercased app identifier (Windows exe file name or
/// macOS bundle id) plus the window title (empty on macOS). Unknown → plain.
#[cfg(any(target_os = "windows", target_os = "macos", test))]
fn classify(app: &str, title: &str) -> OutputFormat {
    // Native rich-text apps: always HTML.
    const RICH_NATIVE: [&str; 10] = [
        "outlook.exe",
        "winword.exe",
        "onenote.exe",
        "hxoutlook.exe", // Windows Mail
        "thunderbird.exe",
        "com.microsoft.outlook",
        "com.microsoft.word",
        "com.apple.mail",
        "com.apple.notes",
        "com.microsoft.onenote.mac",
    ];
    if RICH_NATIVE.contains(&app) {
        return OutputFormat::Html;
    }

    // Browsers: rich only when the page is a known rich webapp. The window
    // title carries the page name on Windows; on macOS we have no title, so
    // browsers fall through to plain text (the intended behaviour there).
    const BROWSERS: [&str; 12] = [
        "chrome.exe",
        "msedge.exe",
        "firefox.exe",
        "brave.exe",
        "opera.exe",
        "vivaldi.exe",
        "arc.exe",
        "com.google.chrome",
        "com.apple.safari",
        "org.mozilla.firefox",
        "company.thebrowser.browser",
        "com.brave.browser",
    ];
    if BROWSERS.contains(&app) {
        let title = title.to_ascii_lowercase();
        let rich_web = ["gmail", "outlook", "google docs"];
        if rich_web.iter().any(|w| title.contains(w)) {
            return OutputFormat::Html;
        }
    }

    OutputFormat::PlainText
}

// ── Windows ────────────────────────────────────────────────────────────────
#[cfg(target_os = "windows")]
fn detect_impl(_app: &tauri::AppHandle) -> OutputFormat {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    };

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd == 0 {
            return OutputFormat::PlainText;
        }

        // Window title (the page name for browsers).
        let title = {
            let len = GetWindowTextLengthW(hwnd);
            if len > 0 {
                let mut buf = vec![0u16; len as usize + 1];
                let copied = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
                String::from_utf16_lossy(&buf[..copied.max(0) as usize])
            } else {
                String::new()
            }
        };

        // Owning process → executable file name.
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return classify("", &title);
        }

        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle == 0 {
            return classify("", &title);
        }

        let exe = {
            let mut buf = vec![0u16; 512];
            let mut size = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
            if ok != 0 {
                String::from_utf16_lossy(&buf[..size as usize])
            } else {
                String::new()
            }
        };
        CloseHandle(handle);

        let exe_name = exe
            .rsplit(|c| c == '\\' || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();

        classify(&exe_name, &title)
    }
}

// ── macOS ──────────────────────────────────────────────────────────────────
// Only the frontmost app's bundle identifier is inspected — reading a browser
// tab's title on macOS needs a permission prompt, so browsers stay plain here.
#[cfg(target_os = "macos")]
fn detect_impl(app: &tauri::AppHandle) -> OutputFormat {
    // The global-shortcut handler (`on_hotkey`/`on_super_hotkey`) is NOT
    // guaranteed to run on the main app thread — calling AppKit's
    // `NSWorkspace` straight from that path caused process-level exits on
    // macOS when the proofread shortcut fired. Marshal the lookup onto the
    // main thread via `run_on_main_thread`, the same pattern
    // `show_processing`/`hide_processing` use, and block on a channel to get
    // the result back synchronously.
    //
    // This is safe even when `detect` happens to already be called from the
    // main thread: Tauri's wry runtime detects that case internally and runs
    // the closure inline before `run_on_main_thread` returns, so the channel
    // send always happens before we start waiting on `recv_timeout` — no
    // deadlock either way. The timeout is just a defensive backstop in case
    // the main loop is ever unavailable (e.g. mid-teardown).
    //
    // UNVERIFIED AT RUNTIME: it's possible the global-shortcut handler that
    // calls `on_hotkey`/`on_super_hotkey` already runs on the main thread on
    // macOS (Carbon's `InstallEventHandler`/`GetApplicationEventTarget`
    // typically dispatch on the main run loop), in which case this
    // `run_on_main_thread` hop is a no-op and does NOT change which thread
    // makes the AppKit call — the original crash may not actually be fixed.
    // The two `crate::trace(...)` calls below (caller thread vs. closure
    // thread, both with `is_main=`) exist so a human running the packaged
    // app can read the trace log and confirm whether they ever differ. See
    // `project.md` Known Gaps for exactly how to check this.
    crate::trace(&format!(
        "foreground::detect_impl: calling thread is_main={}",
        crate::is_main_thread()
    ));
    use objc2_app_kit::NSWorkspace;
    use std::sync::mpsc;
    use std::time::Duration;

    let (tx, rx) = mpsc::channel();
    let posted = app.run_on_main_thread(move || {
        crate::trace(&format!(
            "foreground::detect_impl: run_on_main_thread closure is_main={}",
            crate::is_main_thread()
        ));
        let bundle_id = unsafe {
            NSWorkspace::sharedWorkspace()
                .frontmostApplication()
                .and_then(|running_app| running_app.bundleIdentifier())
                .map(|s| s.to_string())
        };
        let _ = tx.send(bundle_id);
    });

    if posted.is_err() {
        return OutputFormat::PlainText;
    }

    match rx.recv_timeout(Duration::from_millis(500)) {
        Ok(Some(bundle_id)) => classify(&bundle_id.to_ascii_lowercase(), ""),
        _ => OutputFormat::PlainText,
    }
}

// ── Other platforms ────────────────────────────────────────────────────────
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn detect_impl(_app: &tauri::AppHandle) -> OutputFormat {
    OutputFormat::PlainText
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_rich_apps_are_html() {
        assert_eq!(classify("outlook.exe", ""), OutputFormat::Html);
        assert_eq!(classify("winword.exe", ""), OutputFormat::Html);
        assert_eq!(classify("com.apple.mail", ""), OutputFormat::Html);
    }

    #[test]
    fn browser_on_rich_webapp_is_html() {
        assert_eq!(
            classify(
                "chrome.exe",
                "Inbox (2) - me@gmail.com - Gmail - Google Chrome"
            ),
            OutputFormat::Html
        );
        assert_eq!(
            classify("msedge.exe", "Mail - me - Outlook"),
            OutputFormat::Html
        );
    }

    #[test]
    fn browser_on_unknown_site_is_plain() {
        assert_eq!(
            classify("chrome.exe", "Hacker News - Google Chrome"),
            OutputFormat::PlainText
        );
    }

    #[test]
    fn browser_without_title_is_plain() {
        // macOS gives browsers no title, so they must fall through to plain.
        assert_eq!(classify("com.google.chrome", ""), OutputFormat::PlainText);
    }

    #[test]
    fn unknown_app_is_plain() {
        assert_eq!(classify("notepad.exe", ""), OutputFormat::PlainText);
        assert_eq!(classify("", ""), OutputFormat::PlainText);
    }
}
