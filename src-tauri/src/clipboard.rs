use anyhow::Result;
use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::thread;
use std::time::Duration;

// macOS uses Cmd (Meta) for copy/paste; Windows/Linux use Ctrl.
fn copy_paste_mod() -> Key {
    if cfg!(target_os = "macos") {
        Key::Meta
    } else {
        Key::Control
    }
}

fn shortcut_letter_key(c: char) -> Key {
    #[cfg(target_os = "macos")]
    {
        match c {
            // Avoid Key::Unicode on macOS: Enigo resolves it through HIToolbox's
            // current input source APIs, which assert main-queue usage and can
            // crash when capture/paste runs on a Tokio blocking worker.
            'c' | 'C' => Key::Other(8),
            'v' | 'V' => Key::Other(9),
            _ => Key::Unicode(c),
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        Key::Unicode(c)
    }
}

/// Block until the user has physically released every modifier key that could
/// be held from the triggering hotkey, or until `timeout` elapses. This is
/// essential before we synthesize Cmd/Ctrl+C: if the user's physical modifier-up
/// lands in the middle of our synthetic copy chord, the `c` can be seen without
/// its modifier, or as the wrong shifted shortcut.
#[cfg(target_os = "windows")]
fn wait_for_modifiers_release(timeout: Duration) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };

    const KEYS: [i32; 5] = [
        VK_CONTROL as i32,
        VK_SHIFT as i32,
        VK_MENU as i32,
        VK_LWIN as i32,
        VK_RWIN as i32,
    ];

    let deadline = std::time::Instant::now() + timeout;
    loop {
        // High-order bit set means the key is currently physically down.
        let any_down = KEYS
            .iter()
            .any(|&vk| (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0);
        if !any_down || std::time::Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(15));
    }
}

#[cfg(target_os = "macos")]
fn wait_for_modifiers_release(timeout: Duration) {
    type CGKeyCode = u16;
    type CGEventSourceStateID = i32;

    const HID_SYSTEM_STATE: CGEventSourceStateID = 1;
    const KEYS: [CGKeyCode; 8] = [
        55, // left command
        54, // right command
        56, // left shift
        60, // right shift
        58, // left option
        61, // right option
        59, // left control
        62, // right control
    ];

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn CGEventSourceKeyState(state_id: CGEventSourceStateID, key: CGKeyCode) -> bool;
    }

    let deadline = std::time::Instant::now() + timeout;
    loop {
        let any_down = KEYS
            .iter()
            .any(|&key| unsafe { CGEventSourceKeyState(HID_SYSTEM_STATE, key) });
        if !any_down || std::time::Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(15));
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn wait_for_modifiers_release(_timeout: Duration) {}

/// `pub(crate)` (not just module-private) so `commands.rs` can expose it to
/// the frontend via `check_accessibility_permission` / `request_accessibility_permission`
/// (see Phase 2 of `roadmap-mac.md`, the Accessibility permission tutorial).
#[cfg(target_os = "macos")]
pub(crate) fn accessibility_trusted(prompt: bool) -> bool {
    use core::ffi::c_void;

    type CFAllocatorRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFTypeRef = *const c_void;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: CFTypeRef;
        fn CFDictionaryCreate(
            allocator: CFAllocatorRef,
            keys: *const *const c_void,
            values: *const *const c_void,
            num_values: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> CFDictionaryRef;
        fn CFRelease(cf: CFTypeRef);
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
        static kAXTrustedCheckOptionPrompt: CFStringRef;
        fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    }

    if unsafe { AXIsProcessTrusted() } {
        return true;
    }

    if prompt {
        let key = unsafe { kAXTrustedCheckOptionPrompt as *const c_void };
        let value = unsafe { kCFBooleanTrue as *const c_void };
        let options = unsafe {
            CFDictionaryCreate(
                core::ptr::null(),
                &key,
                &value,
                1,
                core::ptr::null(),
                core::ptr::null(),
            )
        };
        if !options.is_null() {
            let _ = unsafe { AXIsProcessTrustedWithOptions(options) };
            unsafe { CFRelease(options as CFTypeRef) };
        }
    }

    unsafe { AXIsProcessTrusted() }
}

#[cfg(target_os = "macos")]
pub(crate) fn accessibility_error_message() -> String {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown executable path".to_string());

    format!(
        "macOS Accessibility permission is still not trusted for this running dev binary:\n{exe}\n\nIn System Settings > Privacy & Security > Accessibility, remove any stale reWrite/rewrite entries, add or enable this exact binary if it appears, then fully quit and restart the dev app."
    )
}

/// Simulate Cmd/Ctrl+C to copy the current selection.
/// Returns (selected_text, previous_clipboard_contents).
pub fn capture_selection() -> Result<(String, String)> {
    #[cfg(target_os = "macos")]
    if !accessibility_trusted(true) {
        anyhow::bail!(accessibility_error_message());
    }

    // The hotkey fires on key-press while the user is still physically holding
    // the modifier(s). Wait for them to let go before we synthesize anything —
    // if a physical Ctrl-up interleaves with our synthetic Ctrl+C, the `c` is
    // typed literally and overwrites the user's selection.
    wait_for_modifiers_release(Duration::from_millis(1000));

    // Belt-and-suspenders: also release the modifiers synthetically so nothing
    // bleeds into the copy on platforms without the wait above.
    let mut enigo = Enigo::new(&Settings::default())?;
    enigo.key(Key::Shift, Direction::Release)?;
    enigo.key(Key::Control, Direction::Release)?;
    #[cfg(target_os = "macos")]
    enigo.key(Key::Meta, Direction::Release)?;
    drop(enigo);

    thread::sleep(Duration::from_millis(100));

    // Save existing clipboard content, then clear it so we can detect a fresh copy.
    let original = {
        let mut cb = Clipboard::new()?;
        let text = cb.get_text().unwrap_or_default();
        let _ = cb.set_text("");
        text
    };

    // Simulate Cmd+C (Mac) or Ctrl+C (Windows/Linux). Press the modifier and
    // give it a beat to register before pressing `c`, so the key is never seen
    // without its modifier.
    let modifier = copy_paste_mod();
    let copy_key = shortcut_letter_key('c');
    let mut enigo = Enigo::new(&Settings::default())?;
    enigo.key(modifier, Direction::Press)?;
    thread::sleep(Duration::from_millis(40));
    enigo.key(copy_key, Direction::Press)?;
    thread::sleep(Duration::from_millis(20));
    enigo.key(copy_key, Direction::Release)?;
    enigo.key(modifier, Direction::Release)?;
    drop(enigo);

    // Wait for the source app to fill the clipboard. Some macOS apps update
    // the pasteboard asynchronously after Cmd+C, so poll briefly instead of
    // assuming a fixed 200ms delay is enough.
    let mut captured = String::new();
    for _ in 0..20 {
        thread::sleep(Duration::from_millis(50));
        captured = Clipboard::new()?.get_text().unwrap_or_default();
        if !captured.is_empty() {
            break;
        }
    }
    if captured.is_empty() && !original.is_empty() {
        let _ = Clipboard::new()?.set_text(original.clone());
    }
    Ok((captured, original))
}

/// Passively read the current clipboard contents — no synthetic Ctrl+C, no
/// focus required. Used by the selection bubble's click handler to snapshot
/// "what was on the clipboard before we touch it", since simulating a copy
/// there would risk racing the click's own focus change (see selection_watcher
/// module docs / the design note in lib.rs for why the bubble never re-copies).
pub fn snapshot_clipboard() -> Result<String> {
    Ok(Clipboard::new()?.get_text().unwrap_or_default())
}

/// Write result to clipboard, simulate Ctrl+V, then optionally restore the original.
pub fn paste_and_restore(
    trace_id: u64,
    result: &str,
    original: &str,
    restore: bool,
    restore_delay_ms: u64,
) -> Result<()> {
    crate::trace(&format!(
        "paste#{trace_id}: paste_and_restore start result={} restore={restore}",
        crate::text_fingerprint(result)
    ));
    Clipboard::new()?.set_text(result)?;
    crate::trace(&format!("paste#{trace_id}: plain clipboard written"));

    thread::sleep(Duration::from_millis(50));

    let modifier = copy_paste_mod();
    let paste_key = shortcut_letter_key('v');
    let mut enigo = Enigo::new(&Settings::default())?;
    enigo.key(modifier, Direction::Press)?;
    // Same settle delay as the copy sequence in `capture_selection`, and for
    // the same reason: without it, the paste-key press can land before the
    // OS/target app has registered the modifier as held, so it's seen as an
    // unmodified keystroke — the target app types a literal "v"/"c" instead
    // of running the paste shortcut, silently discarding the rewritten text
    // that's already sitting in the clipboard right next to it.
    thread::sleep(Duration::from_millis(40));
    enigo.key(paste_key, Direction::Press)?;
    thread::sleep(Duration::from_millis(20));
    enigo.key(paste_key, Direction::Release)?;
    enigo.key(modifier, Direction::Release)?;
    drop(enigo);
    crate::trace(&format!("paste#{trace_id}: synthetic Ctrl+V sent"));

    if restore && !original.is_empty() {
        thread::sleep(Duration::from_millis(restore_delay_ms));
        let _ = Clipboard::new()?.set_text(original);
        crate::trace(&format!(
            "paste#{trace_id}: original clipboard restored len={}",
            original.len()
        ));
    }

    Ok(())
}

/// Like `paste_and_restore`, but writes rich HTML to the clipboard (with a
/// plain-text fallback for apps that only read plain text), then pastes it.
/// arboard maps this to CF_HTML on Windows and `public.html` on macOS.
pub fn paste_html_and_restore(
    trace_id: u64,
    html: &str,
    plain_fallback: &str,
    original: &str,
    restore: bool,
    restore_delay_ms: u64,
) -> Result<()> {
    crate::trace(&format!(
        "paste#{trace_id}: paste_html_and_restore start html={} fallback={} restore={restore}",
        crate::text_fingerprint(html),
        crate::text_fingerprint(plain_fallback)
    ));
    Clipboard::new()?.set().html(html, Some(plain_fallback))?;
    crate::trace(&format!("paste#{trace_id}: HTML clipboard written"));

    thread::sleep(Duration::from_millis(50));

    let modifier = copy_paste_mod();
    let paste_key = shortcut_letter_key('v');
    let mut enigo = Enigo::new(&Settings::default())?;
    enigo.key(modifier, Direction::Press)?;
    // Same settle delay as the copy sequence in `capture_selection`, and for
    // the same reason: without it, the paste-key press can land before the
    // OS/target app has registered the modifier as held, so it's seen as an
    // unmodified keystroke — the target app types a literal "v"/"c" instead
    // of running the paste shortcut, silently discarding the rewritten text
    // that's already sitting in the clipboard right next to it.
    thread::sleep(Duration::from_millis(40));
    enigo.key(paste_key, Direction::Press)?;
    thread::sleep(Duration::from_millis(20));
    enigo.key(paste_key, Direction::Release)?;
    enigo.key(modifier, Direction::Release)?;
    drop(enigo);
    crate::trace(&format!("paste#{trace_id}: synthetic Ctrl+V sent"));

    if restore && !original.is_empty() {
        thread::sleep(Duration::from_millis(restore_delay_ms));
        let _ = Clipboard::new()?.set_text(original);
        crate::trace(&format!(
            "paste#{trace_id}: original clipboard restored len={}",
            original.len()
        ));
    }

    Ok(())
}

/// Produce a readable plain-text rendering of HTML output — used as the
/// clipboard's plain-text fallback and for history/word-count. Block-level
/// closes and `<br>` become newlines, remaining tags are dropped, common
/// entities are decoded, and runs of blank lines are collapsed.
pub fn strip_html_tags(html: &str) -> String {
    // Turn block boundaries into newlines before removing tags.
    let mut s = html
        .replace("</p>", "\n")
        .replace("</li>", "\n")
        .replace("</ul>", "\n")
        .replace("</ol>", "\n")
        .replace("</div>", "\n")
        .replace("</h1>", "\n")
        .replace("</h2>", "\n")
        .replace("</h3>", "\n");
    for br in ["<br>", "<br/>", "<br />"] {
        s = s.replace(br, "\n");
    }

    // Drop every remaining tag.
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }

    // Decode the handful of entities we're likely to emit. `&amp;` last so an
    // already-decoded `&` isn't produced mid-stream and re-decoded.
    out = out
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");

    // Collapse 3+ newlines to a paragraph break and trim surrounding blanks.
    let mut collapsed = String::with_capacity(out.len());
    let mut newline_run = 0;
    for c in out.chars() {
        if c == '\n' {
            newline_run += 1;
            if newline_run <= 2 {
                collapsed.push(c);
            }
        } else {
            newline_run = 0;
            collapsed.push(c);
        }
    }
    collapsed.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::strip_html_tags;

    #[test]
    fn paragraphs_become_blank_line_separated() {
        assert_eq!(
            strip_html_tags("<p>Hello there.</p><p>Second para.</p>"),
            "Hello there.\nSecond para."
        );
    }

    #[test]
    fn list_items_become_lines() {
        assert_eq!(
            strip_html_tags("<ul><li>One</li><li>Two</li></ul>"),
            "One\nTwo"
        );
    }

    #[test]
    fn inline_tags_are_dropped_text_kept() {
        assert_eq!(
            strip_html_tags("A <strong>bold</strong> and <em>italic</em> word"),
            "A bold and italic word"
        );
    }

    #[test]
    fn entities_are_decoded() {
        assert_eq!(
            strip_html_tags("<p>Tom &amp; Jerry said &quot;hi&quot; &lt;3</p>"),
            "Tom & Jerry said \"hi\" <3"
        );
    }

    #[test]
    fn br_becomes_newline() {
        assert_eq!(
            strip_html_tags("line one<br>line two"),
            "line one\nline two"
        );
    }
}
