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
pub fn detect() -> OutputFormat {
    detect_impl()
}

/// Decide the format from a lowercased app identifier (Windows exe file name or
/// macOS bundle id) plus the window title (empty on macOS). Unknown → plain.
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
        "com.onenote.mac",
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
fn detect_impl() -> OutputFormat {
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
fn detect_impl() -> OutputFormat {
    use objc2_app_kit::NSWorkspace;

    unsafe {
        let workspace = NSWorkspace::sharedWorkspace();
        let Some(app) = workspace.frontmostApplication() else {
            return OutputFormat::PlainText;
        };
        match app.bundleIdentifier() {
            Some(bundle) => classify(&bundle.to_string().to_ascii_lowercase(), ""),
            None => OutputFormat::PlainText,
        }
    }
}

// ── Other platforms ────────────────────────────────────────────────────────
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn detect_impl() -> OutputFormat {
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
