use anyhow::Result;
use arboard::Clipboard;
#[cfg(not(target_os = "macos"))]
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::thread;
use std::time::Duration;

// macOS uses Cmd (Meta) for copy/paste; Windows/Linux use Ctrl.
#[cfg(not(target_os = "macos"))]
fn copy_paste_mod() -> Key {
    Key::Control
}

#[cfg(target_os = "macos")]
mod macos_keys {
    use anyhow::{anyhow, Result};
    use std::ffi::c_void;
    use std::thread;
    use std::time::Duration;

    const KEY_C: u16 = 8;
    const KEY_V: u16 = 9;
    const CG_HID_EVENT_TAP: u32 = 0;
    const CG_EVENT_FLAG_MASK_COMMAND: u64 = 0x0010_0000;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn CGEventCreateKeyboardEvent(
            source: *mut c_void,
            virtual_key: u16,
            key_down: bool,
        ) -> *mut c_void;
        fn CGEventSetFlags(event: *mut c_void, flags: u64);
        fn CGEventPost(tap: u32, event: *mut c_void);
        fn CFRelease(cf: *const c_void);
    }

    fn post_key(virtual_key: u16, key_down: bool, flags: u64) -> Result<()> {
        let event =
            unsafe { CGEventCreateKeyboardEvent(std::ptr::null_mut(), virtual_key, key_down) };
        if event.is_null() {
            return Err(anyhow!("CGEventCreateKeyboardEvent failed"));
        }

        unsafe {
            CGEventSetFlags(event, flags);
            CGEventPost(CG_HID_EVENT_TAP, event);
            CFRelease(event as *const c_void);
        }
        Ok(())
    }

    fn command_shortcut(virtual_key: u16) -> Result<()> {
        post_key(virtual_key, true, CG_EVENT_FLAG_MASK_COMMAND)?;
        thread::sleep(Duration::from_millis(20));
        post_key(virtual_key, false, CG_EVENT_FLAG_MASK_COMMAND)
    }

    pub fn copy() -> Result<()> {
        command_shortcut(KEY_C)
    }

    pub fn paste() -> Result<()> {
        command_shortcut(KEY_V)
    }
}

/// Block until the user has physically released every modifier key that could
/// be held from the triggering hotkey (Ctrl/Shift/Alt/Win), or until `timeout`
/// elapses. This is essential before we synthesize Ctrl+C: if the user's
/// physical Ctrl-up lands in the middle of our synthetic Ctrl+C, the `c` is
/// seen without a modifier and gets typed as a literal character — overwriting
/// the user's selection. Waiting for release closes that race.
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

/// Simulate Cmd/Ctrl+C to copy the current selection.
/// Returns (selected_text, previous_clipboard_contents).
pub fn capture_selection() -> Result<(String, String)> {
    // The hotkey fires on key-press while the user is still physically holding
    // the modifier(s). Wait for them to let go before we synthesize anything —
    // if a physical Ctrl-up interleaves with our synthetic Ctrl+C, the `c` is
    // typed literally and overwrites the user's selection.
    #[cfg(target_os = "windows")]
    wait_for_modifiers_release(Duration::from_millis(1000));

    // Belt-and-suspenders: also release the modifiers synthetically so nothing
    // bleeds into the copy on platforms without the wait above. Do not use
    // enigo for this on macOS: its layout lookup calls HIToolbox APIs that
    // assert when reached from the Tokio worker thread that runs this capture.
    #[cfg(not(target_os = "macos"))]
    {
        let mut enigo = Enigo::new(&Settings::default())?;
        enigo.key(Key::Shift, Direction::Release)?;
        enigo.key(Key::Control, Direction::Release)?;
        drop(enigo);
    }

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
    #[cfg(target_os = "macos")]
    macos_keys::copy()?;

    #[cfg(not(target_os = "macos"))]
    {
        let modifier = copy_paste_mod();
        let mut enigo = Enigo::new(&Settings::default())?;
        enigo.key(modifier, Direction::Press)?;
        thread::sleep(Duration::from_millis(40));
        enigo.key(Key::Unicode('c'), Direction::Press)?;
        thread::sleep(Duration::from_millis(20));
        enigo.key(Key::Unicode('c'), Direction::Release)?;
        enigo.key(modifier, Direction::Release)?;
        drop(enigo);
    }

    // Wait for the source app to fill the clipboard.
    thread::sleep(Duration::from_millis(200));

    let captured = Clipboard::new()?.get_text().unwrap_or_default();
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

    #[cfg(target_os = "macos")]
    macos_keys::paste()?;

    #[cfg(not(target_os = "macos"))]
    {
        let modifier = copy_paste_mod();
        let mut enigo = Enigo::new(&Settings::default())?;
        enigo.key(modifier, Direction::Press)?;
        enigo.key(Key::Unicode('v'), Direction::Click)?;
        enigo.key(modifier, Direction::Release)?;
        drop(enigo);
    }
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

    #[cfg(target_os = "macos")]
    macos_keys::paste()?;

    #[cfg(not(target_os = "macos"))]
    {
        let modifier = copy_paste_mod();
        let mut enigo = Enigo::new(&Settings::default())?;
        enigo.key(modifier, Direction::Press)?;
        enigo.key(Key::Unicode('v'), Direction::Click)?;
        enigo.key(modifier, Direction::Release)?;
        drop(enigo);
    }
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
