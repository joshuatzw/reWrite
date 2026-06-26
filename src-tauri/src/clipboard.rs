use anyhow::Result;
use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::thread;
use std::time::Duration;

// macOS uses Cmd (Meta) for copy/paste; Windows/Linux use Ctrl.
fn copy_paste_mod() -> Key {
    if cfg!(target_os = "macos") { Key::Meta } else { Key::Control }
}

/// Simulate Cmd/Ctrl+C to copy the current selection.
/// Returns (selected_text, previous_clipboard_contents).
pub fn capture_selection() -> Result<(String, String)> {
    // Release the hotkey modifiers so the source app doesn't see them bleed into the copy.
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

    // Simulate Cmd+C (Mac) or Ctrl+C (Windows/Linux).
    let modifier = copy_paste_mod();
    let mut enigo = Enigo::new(&Settings::default())?;
    enigo.key(modifier, Direction::Press)?;
    enigo.key(Key::Unicode('c'), Direction::Click)?;
    enigo.key(modifier, Direction::Release)?;
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

    let modifier = copy_paste_mod();
    let mut enigo = Enigo::new(&Settings::default())?;
    enigo.key(modifier, Direction::Press)?;
    enigo.key(Key::Unicode('v'), Direction::Click)?;
    enigo.key(modifier, Direction::Release)?;
    drop(enigo);

    if restore && !original.is_empty() {
        thread::sleep(Duration::from_millis(restore_delay_ms));
        let _ = Clipboard::new()?.set_text(original);
    }

    Ok(())
}
