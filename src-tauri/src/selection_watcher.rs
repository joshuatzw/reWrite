//! Always-on background watcher for the v1.1.0 "selection bubble" feature
//! (Sprint 1: detection only, no UI). Installs a `WH_MOUSE_LL` hook to notice
//! drag-releases, then off that thread — debounced — asks Windows UI
//! Automation whether the focused element now has a non-empty text selection.
//! Emits `selection:detected` / `selection:cleared` for a later sprint's
//! bubble window to consume.
//!
//! Ported from the Sprint 0 feasibility spike at
//! `src-tauri/examples/uia_probe.rs` — see that file for the fuller rationale
//! behind the `windows`-crate choice, the MTA COM apartment, and the
//! fullscreen-exclusive heuristic. Structurally this mirrors `esc_hook.rs`
//! (idempotent start/stop, dedicated hook thread pumping `GetMessageW`,
//! `WM_QUIT` teardown) with one addition: a second worker thread, since the
//! hook callback here must never do the UIA call itself (this hook fires for
//! every mouse event system-wide, including high-frequency drag in games and
//! creative apps).

use std::sync::{
    atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering},
    mpsc, Mutex, OnceLock,
};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

use windows::core::Result as WinResult;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetLBound, SafeArrayGetUBound,
    SafeArrayUnaccessData,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, UIA_TextPatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowRect, IsWindow, SetForegroundWindow,
};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, MSLLHOOKSTRUCT, PostThreadMessageW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, MSG, WH_KEYBOARD_LL, WH_MOUSE_LL,
    WM_KEYUP, WM_LBUTTONUP, WM_QUIT, WM_SYSKEYUP,
};

/// Selections shorter than this (after trimming whitespace) are treated as
/// "no selection" — noise reduction, not a config option. A non-empty but
/// trivially short `TextPattern` selection (a single incidental character
/// grabbed mid-click/drag during normal use) isn't a real "rewrite this"
/// signal worth popping the bubble for.
const MIN_SELECTION_CHARS: usize = 2;

static HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static APP: OnceLock<AppHandle> = OnceLock::new();
/// Current worker's notification channel. Replaced on every `start()` so a
/// stop()/start() cycle gets a fresh, correctly-paired channel and worker
/// thread rather than reusing one whose receiver already exited.
static NOTIFY_TX: Mutex<Option<mpsc::Sender<WorkerMsg>>> = Mutex::new(None);
/// Mirrors the worker's `had_selection` so the (hot, system-wide) keyboard
/// hook can skip enqueueing entirely when there's no selection to lose —
/// otherwise every keystroke during normal typing/gaming would round-trip
/// through the channel for nothing. Only once a selection exists does it make
/// sense to also watch for a keyboard edit (Delete/Backspace/typing over the
/// selection) that clears it without any further mouse event.
static HAD_SELECTION: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
enum WorkerMsg {
    Recheck,
    Shutdown,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct AnchorPayload {
    pub x: f64,
    pub y: f64,
    pub text: String,
}

/// The most recently detected selection, mirrored here so `bubble_clicked`
/// can read it directly instead of trusting the frontend to have already
/// received the `selection:detected` event by click time. A live trace
/// session showed the frontend's listener hadn't caught up ~300ms after the
/// event was emitted and the bubble window made visible — plausibly because
/// event delivery into a webview that was hidden a moment earlier is
/// throttled/delayed, and a human click can beat it. Reading Rust-side state
/// instead sidesteps that race entirely.
static LAST_ANCHOR: Mutex<Option<AnchorPayload>> = Mutex::new(None);
static LAST_SOURCE_HWND: AtomicIsize = AtomicIsize::new(0);

pub fn last_anchor() -> Option<AnchorPayload> {
    LAST_ANCHOR.lock().unwrap().clone()
}

/// Best-effort focus restore for the passive bubble paste path. The bubble
/// menu steals foreground; before sending Ctrl+V we need to put focus back on
/// the app that owned the UIA selection.
pub fn focus_last_source_window() -> bool {
    let hwnd = LAST_SOURCE_HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        crate::trace("focus_last_source_window: no hwnd stored");
        return false;
    }

    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            crate::trace("focus_last_source_window: hwnd no longer valid");
            return false;
        }
        let ok = SetForegroundWindow(hwnd).as_bool();
        crate::trace(&format!("focus_last_source_window: set_foreground={ok}"));
        ok
    }
}

/// Install the mouse hook and start the probe worker if not already running.
/// Idempotent, mirroring `esc_hook::start` — safe to call more than once.
pub fn start(app: &AppHandle) {
    let _ = APP.get_or_init(|| app.clone());
    if HOOK_THREAD_ID.load(Ordering::SeqCst) != 0 {
        crate::trace("selection_watcher::start: already running");
        return;
    }

    let (tx, rx) = mpsc::channel();
    *NOTIFY_TX.lock().unwrap() = Some(tx);

    crate::trace("selection_watcher::start: spawning worker thread");
    std::thread::spawn(move || worker_loop(rx));

    crate::trace("selection_watcher::start: spawning hook thread");
    std::thread::spawn(run_hook_thread);
}

/// Uninstall the hook and stop the worker. Not currently called anywhere
/// (this watcher is always-on for the app's lifetime — see module docs) but
/// provided for symmetry with `esc_hook` and for future use (Sprint 4's
/// config toggle).
pub fn stop() {
    let tid = HOOK_THREAD_ID.swap(0, Ordering::SeqCst);
    if tid != 0 {
        unsafe { PostThreadMessageW(tid, WM_QUIT, 0, 0) };
    }
    if let Some(tx) = NOTIFY_TX.lock().unwrap().take() {
        let _ = tx.send(WorkerMsg::Shutdown);
    }
}

fn run_hook_thread() {
    unsafe {
        HOOK_THREAD_ID.store(GetCurrentThreadId(), Ordering::SeqCst);

        let mouse_hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), 0, 0);
        let keyboard_hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), 0, 0);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        if mouse_hook != 0 {
            UnhookWindowsHookEx(mouse_hook);
        }
        if keyboard_hook != 0 {
            UnhookWindowsHookEx(keyboard_hook);
        }
    }
}

/// Absolute minimum work on the hook thread: notice a drag-release and hand
/// off. Every other mouse message (crucially `WM_MOUSEMOVE`, by far the
/// highest-frequency one) falls straight through to `CallNextHookEx` without
/// touching the mutex at all. Never call UIA from here — see module docs.
///
/// Also drives "click outside the bubble menu closes it": a `GetWindowRect`-
/// style bounds check against our own window is cheap and doesn't touch
/// another process, unlike the UIA probe, so it's fine to dispatch (onto the
/// main thread) straight from here rather than through the debounced worker.
unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam == WM_LBUTTONUP as usize {
        if let Some(app) = APP.get() {
            let info = &*(lparam as *const MSLLHOOKSTRUCT);
            crate::maybe_close_bubble_menu_on_outside_click(app, info.pt.x, info.pt.y);
        }
        if let Some(tx) = NOTIFY_TX.lock().unwrap().as_ref() {
            let _ = tx.send(WorkerMsg::Recheck);
        }
    }
    CallNextHookEx(0, code, wparam, lparam)
}

/// Catches the case a mouse-up alone can't: the user deletes (or types over)
/// the selected text via keyboard, with no further mouse event to trigger a
/// recheck. Gated on `HAD_SELECTION` so normal typing/gaming — no selection,
/// hence nothing to lose — never touches the mutex.
unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0
        && (wparam == WM_KEYUP as usize || wparam == WM_SYSKEYUP as usize)
        && HAD_SELECTION.load(Ordering::Relaxed)
    {
        if let Some(tx) = NOTIFY_TX.lock().unwrap().as_ref() {
            let _ = tx.send(WorkerMsg::Recheck);
        }
    }
    CallNextHookEx(0, code, wparam, lparam)
}

fn worker_loop(rx: mpsc::Receiver<WorkerMsg>) {
    // MTA, not STA: this thread never pumps a Win32 message loop, and an STA
    // GetFocusedElement() call into another process's UIA provider relies on
    // message pumping to marshal — without one it can hang. See uia_probe.rs.
    if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
        crate::trace("selection_watcher::worker_loop: CoInitializeEx failed, exiting");
        return;
    }

    let automation: IUIAutomation =
        match unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) } {
            Ok(a) => a,
            Err(_) => {
                unsafe { CoUninitialize() };
                return;
            }
        };

    let mut had_selection = false;

    'outer: while let Ok(msg) = rx.recv() {
        if matches!(msg, WorkerMsg::Shutdown) {
            break;
        }

        // Debounce: collapse a burst of mouse-ups/key-ups (rapid clicking or
        // dragging, e.g. RTS unit-boxing; or holding Backspace) into a single
        // probe ~200ms after the last one.
        loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(WorkerMsg::Recheck) => continue,
                Ok(WorkerMsg::Shutdown) => break 'outer,
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break 'outer,
            }
        }

        run_probe_cycle(&automation, &mut had_selection);
    }

    unsafe { CoUninitialize() };
}

fn run_probe_cycle(automation: &IUIAutomation, had_selection: &mut bool) {
    // Skip the (otherwise free) UIA call entirely for fullscreen-exclusive
    // foreground apps — games doing constant click-drag are exactly the case
    // this debounce/off-thread design exists to not add latency to.
    if is_foreground_fullscreen_exclusive().unwrap_or(false) {
        return;
    }

    // While the bubble's skill menu is open the user is mid-interaction with
    // our own popup. Clicking the bubble is itself a mouse-up the hook above
    // sees, and the source app's selection is typically still logically
    // present (most apps don't clear a selection just because a different
    // window took focus) — so without this guard, the debounced recheck that
    // follows the click re-detects that same selection and re-emits
    // `selection:detected`, popping the tiny bubble right back up on top of
    // the menu that was just opened. Skip entirely until the menu closes.
    if bubble_menu_is_open() {
        crate::trace("run_probe_cycle: skipped, bubble_menu open/recently closed");
        return;
    }

    let probe_result = probe_selection(automation);
    crate::trace(&format!(
        "run_probe_cycle: probe_selection -> {}",
        match &probe_result {
            Ok(Some((text, rects))) => format!("Some({} chars, {} rects)", text.trim().chars().count(), rects.len()),
            Ok(None) => "None".to_string(),
            Err(e) => format!("Err({e:?})"),
        }
    ));

    match probe_result.unwrap_or(None) {
        // Too-short selections (after trimming) fall through to the `_` arm
        // below — same "no selection" treatment as an empty/absent one, so
        // they neither emit `detected` nor incorrectly suppress a later
        // `cleared` once the selection eventually does go away.
        Some((text, rects))
            if !rects.is_empty() && text.trim().chars().count() >= MIN_SELECTION_CHARS =>
        {
            *had_selection = true;
            HAD_SELECTION.store(true, Ordering::Relaxed);
            let (x, y) = selection_anchor(&rects);
            let payload = AnchorPayload { x, y, text };
            *LAST_ANCHOR.lock().unwrap() = Some(payload.clone());
            LAST_SOURCE_HWND.store(
                unsafe { GetForegroundWindow().0 as isize },
                Ordering::SeqCst,
            );
            crate::trace(&format!("run_probe_cycle: emitting selection:detected at ({x}, {y})"));
            if let Some(app) = APP.get() {
                let _ = app.emit("selection:detected", payload);
            }
        }
        _ => {
            if *had_selection {
                *had_selection = false;
                HAD_SELECTION.store(false, Ordering::Relaxed);
                *LAST_ANCHOR.lock().unwrap() = None;
                LAST_SOURCE_HWND.store(0, Ordering::SeqCst);
                crate::trace("run_probe_cycle: emitting selection:cleared");
                if let Some(app) = APP.get() {
                    let _ = app.emit("selection:cleared", ());
                }
            }
        }
    }
}

/// Whether the bubble's skill-menu popup window is currently shown or was just
/// dismissed. See the call site in `run_probe_cycle` for why this matters: the
/// mouse-up that closes the menu should not immediately re-probe the still-
/// selected source text and pop the bubble back up.
fn bubble_menu_is_open() -> bool {
    crate::bubble_menu_probe_suppressed()
        || APP
            .get()
            .and_then(|app| app.get_webview_window("bubble_menu"))
            .and_then(|w| w.outer_position().ok())
            .map(|pos| !crate::is_bubble_menu_parked(pos.x, pos.y))
            .unwrap_or(false)
}

/// Anchor = end of the selection, i.e. the bottom-right corner of the last
/// bounding rect (rects are in visual order per UIA's `GetBoundingRectangles`).
fn selection_anchor(rects: &[(f64, f64, f64, f64)]) -> (f64, f64) {
    let (left, top, width, height) = *rects.last().expect("caller checked non-empty");
    (left + width, top + height)
}

/// `Some((selected_text, bounding_rects))` if the focused element supports
/// TextPattern and currently has a non-empty selection, `None` otherwise
/// (including "focused element doesn't support text at all", which is exactly
/// what filters out RTS unit-boxing / file-manager drag-drop).
fn probe_selection(
    automation: &IUIAutomation,
) -> WinResult<Option<(String, Vec<(f64, f64, f64, f64)>)>> {
    // Any error here (nothing focused, inaccessible tree, ...) means "no
    // selection" rather than a fatal error — this runs every debounce cycle
    // against whatever app happens to be foreground.
    let element = match unsafe { automation.GetFocusedElement() } {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    let text_pattern: IUIAutomationTextPattern =
        match unsafe { element.GetCurrentPatternAs(UIA_TextPatternId) } {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };

    let selection = unsafe { text_pattern.GetSelection() }?;
    let count = unsafe { selection.Length() }?;
    if count <= 0 {
        return Ok(None);
    }

    let mut combined_text = String::new();
    let mut rects = Vec::new();

    for i in 0..count {
        let range = unsafe { selection.GetElement(i) }?;

        let bstr = unsafe { range.GetText(-1) }?;
        let text = bstr.to_string();
        if !text.is_empty() {
            combined_text.push_str(&text);
        }

        let safearray = unsafe { range.GetBoundingRectangles() }?;
        if !safearray.is_null() {
            let parsed = unsafe { parse_rect_safearray(safearray) };
            unsafe { SafeArrayDestroy(safearray).ok() };
            rects.extend(parsed?);
        }
    }

    if combined_text.is_empty() {
        Ok(None)
    } else {
        Ok(Some((combined_text, rects)))
    }
}

/// UIA's `GetBoundingRectangles` returns a flat SAFEARRAY of f64s, 4 per
/// rectangle (left, top, width, height) — a selection can span multiple
/// visual lines/rects, hence the flattening.
unsafe fn parse_rect_safearray(
    sa: *mut windows::Win32::System::Com::SAFEARRAY,
) -> WinResult<Vec<(f64, f64, f64, f64)>> {
    let lbound = SafeArrayGetLBound(sa, 1)?;
    let ubound = SafeArrayGetUBound(sa, 1)?;
    if ubound < lbound {
        return Ok(Vec::new());
    }
    let elem_count = (ubound - lbound + 1) as usize;

    let mut data_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
    SafeArrayAccessData(sa, &mut data_ptr)?;
    let slice = std::slice::from_raw_parts(data_ptr as *const f64, elem_count);
    let rects = slice
        .chunks_exact(4)
        .map(|c| (c[0], c[1], c[2], c[3]))
        .collect();
    SafeArrayUnaccessData(sa)?;

    Ok(rects)
}

/// Heuristic: the foreground window is (probably) a fullscreen-exclusive app
/// (typical of games) if its window rect exactly matches its monitor's full
/// rect. Intentionally crude — see uia_probe.rs for the full rationale on the
/// acceptable false positives (borderless-maximized media players/browsers).
fn is_foreground_fullscreen_exclusive() -> WinResult<bool> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return Ok(false);
        }

        let mut window_rect = RECT::default();
        GetWindowRect(hwnd, &mut window_rect)?;

        let hmonitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut monitor_info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(hmonitor, &mut monitor_info).ok()?;

        Ok(window_rect.left == monitor_info.rcMonitor.left
            && window_rect.top == monitor_info.rcMonitor.top
            && window_rect.right == monitor_info.rcMonitor.right
            && window_rect.bottom == monitor_info.rcMonitor.bottom)
    }
}
