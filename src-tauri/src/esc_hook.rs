use std::sync::{
    atomic::{AtomicIsize, AtomicU32, Ordering},
    OnceLock,
};
// AtomicIsize used for HOOK handle (HHOOK is isize on Windows)
use tauri::{AppHandle, Manager};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

static HOOK: AtomicIsize = AtomicIsize::new(0);
static HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static APP: OnceLock<AppHandle> = OnceLock::new();

/// Install the Esc hook. The hook is only active while the overlay is visible;
/// it hides the overlay on any Escape keypress without checking the foreground
/// window (SetForegroundWindow can silently fail, making HWND comparisons
/// unreliable for programmatically-shown windows).
pub fn start(app: &AppHandle) {
    stop();
    let _ = APP.get_or_init(|| app.clone());
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

        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), 0, 0);
        HOOK.store(hook as isize, Ordering::SeqCst);

        // Pump messages so the hook callback fires on this thread.
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // WM_QUIT received — uninstall hook and exit.
        let h = HOOK.swap(0, Ordering::SeqCst);
        if h != 0 {
            UnhookWindowsHookEx(h as HHOOK);
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
                        let _ = w.hide();
                        return 1; // consume the keypress
                    }
                }
            }
        }
    }
    CallNextHookEx(0, code, wparam, lparam)
}
