use std::sync::{
    atomic::{AtomicU32, Ordering},
    OnceLock,
};
use tauri::{AppHandle, Emitter, Manager};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

static HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static APP: OnceLock<AppHandle> = OnceLock::new();

/// Install the Esc hook if it isn't already running. The hook is only active
/// while the overlay is visible; it hides the overlay on any Escape keypress
/// without checking the foreground window (SetForegroundWindow can silently
/// fail, making HWND comparisons unreliable for programmatically-shown windows).
///
/// This is idempotent by design: it must be safe to call on every overlay show.
/// Tearing down and respawning the hook thread on each call previously caused a
/// race (two threads touching one shared HOOK handle) that could silently
/// unhook the freshly-installed hook, leaving Escape non-functional.
pub fn start(app: &AppHandle) {
    let _ = APP.get_or_init(|| app.clone());
    if HOOK_THREAD_ID.load(Ordering::SeqCst) != 0 {
        return; // already running
    }
    std::thread::spawn(run_hook_thread);
}

/// Uninstall the hook by posting WM_QUIT to the hook thread.
pub fn stop() {
    let tid = HOOK_THREAD_ID.swap(0, Ordering::SeqCst);
    if tid != 0 {
        unsafe { PostThreadMessageW(tid, WM_QUIT, 0, 0) };
    }
}

fn run_hook_thread() {
    unsafe {
        HOOK_THREAD_ID.store(GetCurrentThreadId(), Ordering::SeqCst);

        // `hook` is a local — only this thread ever unhooks it, so there is no
        // shared handle for a concurrent start()/stop() to race on.
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), 0, 0);

        // Pump messages so the hook callback fires on this thread.
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // WM_QUIT received — uninstall hook and exit. HOOK_THREAD_ID was
        // already zeroed by stop() before it posted WM_QUIT, so it's not
        // touched here (a fast restart may have already stored a newer tid).
        if hook != 0 {
            UnhookWindowsHookEx(hook);
        }
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam == WM_KEYDOWN as usize {
        let kb = &*(lparam as *const KBDLLHOOKSTRUCT);
        if kb.vkCode == 0x1B {
            // VK_ESCAPE — hide overlay if it is currently visible.
            // The hook is only installed while the overlay is shown, so any
            // Escape press at this point should dismiss it.
            if let Some(app) = APP.get() {
                if let Some(w) = app.get_webview_window("overlay") {
                    if w.is_visible().unwrap_or(false) {
                        stop();
                        // Forward Esc to the overlay's own close handler (the
                        // same one the X button uses) instead of hiding from
                        // this hook thread. A hide issued here runs
                        // ShowWindow(SW_HIDE) from a thread that doesn't own the
                        // window; Windows ignores that for a foreground window,
                        // so Esc silently did nothing whenever the overlay itself
                        // had focus. Routing through JS makes the hide run on the
                        // window's owning main thread, which works regardless of
                        // focus.
                        let _ = app.emit_to("overlay", "overlay:esc", ());
                        return 1; // consume the keypress
                    }
                }
            }
        }
    }
    CallNextHookEx(0, code, wparam, lparam)
}
