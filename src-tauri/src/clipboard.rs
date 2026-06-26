use anyhow::Result;
use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::thread;
use std::time::Duration;

/// Simulate Ctrl+C to copy the current selection.
/// Returns (selected_text, previous_clipboard_contents).
pub fn capture_selection() -> Result<(String, String)> {
    // Release Ctrl+Shift so the source app doesn't see Ctrl+Shift+C.
    let mut enigo = Enigo::new(&Settings::default())?;
    enigo.key(Key::Shift, Direction::Release)?;
    enigo.key(Key::Control, Direction::Release)?;
    drop(enigo);

    thread::sleep(Duration::from_millis(100));

    // Save existing clipboard content, then clear it so we can detect a fresh copy.
    let original = {
        let mut cb = Clipboard::new()?;
        let text = cb.get_text().unwrap_or_default();
        let _ = cb.set_text("");
        text
    };

    // Simulate Ctrl+C.
    let mut enigo = Enigo::new(&Settings::default())?;
    enigo.key(Key::Control, Direction::Press)?;
    enigo.key(Key::Unicode('c'), Direction::Click)?;
    enigo.key(Key::Control, Direction::Release)?;
    drop(enigo);

    // Wait for the source app to fill the clipboard.
    thread::sleep(Duration::from_millis(200));

    let captured = Clipboard::new()?.get_text().unwrap_or_default();
    Ok((captured, original))
}

/// Write result to clipboard, simulate Ctrl+V, then optionally restore the original.
pub fn paste_and_restore(
    result: &str,
    original: &str,
    restore: bool,
    restore_delay_ms: u64,
) -> Result<()> {
    Clipboard::new()?.set_text(result)?;

    thread::sleep(Duration::from_millis(50));

    let mut enigo = Enigo::new(&Settings::default())?;
    enigo.key(Key::Control, Direction::Press)?;
    enigo.key(Key::Unicode('v'), Direction::Click)?;
    enigo.key(Key::Control, Direction::Release)?;
    drop(enigo);

    if restore && !original.is_empty() {
        thread::sleep(Duration::from_millis(restore_delay_ms));
        let _ = Clipboard::new()?.set_text(original);
    }

    Ok(())
}
