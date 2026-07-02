use anyhow::Result;
use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::thread;
use std::time::Duration;

// macOS uses Cmd (Meta) for copy/paste; Windows/Linux use Ctrl.
fn copy_paste_mod() -> Key {
    if cfg!(target_os = "macos") { Key::Meta } else { Key::Control }
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
    let mut enigo = Enigo::new(&Settings::default())?;
    enigo.key(modifier, Direction::Press)?;
    thread::sleep(Duration::from_millis(40));
    enigo.key(Key::Unicode('c'), Direction::Press)?;
    thread::sleep(Duration::from_millis(20));
    enigo.key(Key::Unicode('c'), Direction::Release)?;
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
