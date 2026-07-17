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

#[cfg(target_os = "windows")]
mod win {
    use std::sync::{
        atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU32, Ordering},
        mpsc, Mutex, OnceLock,
    };
    use std::time::Duration;
    use tauri::{AppHandle, Emitter, Manager};

    use windows::core::Result as WinResult;
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Ole::{
        SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetLBound, SafeArrayGetUBound,
        SafeArrayUnaccessData,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
        IUIAutomationValuePattern, UIA_TextPatternId, UIA_ValuePatternId,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowRect, IsWindow, SetForegroundWindow,
    };

    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_MENU};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
        TranslateMessage, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
        WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_QUIT,
        WM_SYSKEYDOWN, WM_SYSKEYUP,
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
    static LAST_MOUSE_DOWN_X: AtomicI32 = AtomicI32::new(0);
    static LAST_MOUSE_DOWN_Y: AtomicI32 = AtomicI32::new(0);
    static LAST_MOUSE_UP_X: AtomicI32 = AtomicI32::new(0);
    static LAST_MOUSE_UP_Y: AtomicI32 = AtomicI32::new(0);

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
    unsafe extern "system" fn mouse_hook_proc(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if code >= 0 {
            let info = &*(lparam as *const MSLLHOOKSTRUCT);
            if wparam == WM_LBUTTONDOWN as usize {
                LAST_MOUSE_DOWN_X.store(info.pt.x, Ordering::Relaxed);
                LAST_MOUSE_DOWN_Y.store(info.pt.y, Ordering::Relaxed);
            } else if wparam == WM_LBUTTONUP as usize {
                LAST_MOUSE_UP_X.store(info.pt.x, Ordering::Relaxed);
                LAST_MOUSE_UP_Y.store(info.pt.y, Ordering::Relaxed);

                let dx = info.pt.x - LAST_MOUSE_DOWN_X.load(Ordering::Relaxed);
                let dy = info.pt.y - LAST_MOUSE_DOWN_Y.load(Ordering::Relaxed);
                let was_drag = dx.abs() > 8 || dy.abs() > 8;
                if let Some(app) = APP.get() {
                    crate::maybe_close_bubble_menu_on_outside_click(
                        app, info.pt.x, info.pt.y, was_drag,
                    );
                }
                if let Some(tx) = NOTIFY_TX.lock().unwrap().as_ref() {
                    let _ = tx.send(WorkerMsg::Recheck);
                }
            }
        }
        CallNextHookEx(0, code, wparam, lparam)
    }

    /// Catches keyboard-only selection changes that no mouse-up can observe:
    /// Ctrl+A can create a selection when `HAD_SELECTION` is still false, while
    /// Delete/Backspace/typing can clear a known selection with no later mouse
    /// event. Only Ctrl+A bypasses the `HAD_SELECTION` gate; other keyboard
    /// traffic stays cheap during normal typing/gaming.
    unsafe extern "system" fn keyboard_hook_proc(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if code >= 0 && should_recheck_for_keyboard_event(wparam, lparam) {
            if let Some(tx) = NOTIFY_TX.lock().unwrap().as_ref() {
                let _ = tx.send(WorkerMsg::Recheck);
            }
        }
        CallNextHookEx(0, code, wparam, lparam)
    }

    unsafe fn should_recheck_for_keyboard_event(wparam: WPARAM, lparam: LPARAM) -> bool {
        if wparam == WM_KEYDOWN as usize || wparam == WM_SYSKEYDOWN as usize {
            let kb = &*(lparam as *const KBDLLHOOKSTRUCT);
            if kb.vkCode == b'A' as u32 && is_ctrl_down_without_alt() {
                crate::trace("selection_watcher::keyboard_hook_proc: Ctrl+A recheck");
                return true;
            }
        }

        (wparam == WM_KEYUP as usize || wparam == WM_SYSKEYUP as usize)
            && HAD_SELECTION.load(Ordering::Relaxed)
    }

    fn is_ctrl_down_without_alt() -> bool {
        let ctrl_down = (unsafe { GetAsyncKeyState(VK_CONTROL as i32) } as u16 & 0x8000) != 0;
        let alt_down = (unsafe { GetAsyncKeyState(VK_MENU as i32) } as u16 & 0x8000) != 0;
        ctrl_down && !alt_down
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
                Ok(Some((text, rects))) => format!(
                    "Some({} chars, {} rects)",
                    text.trim().chars().count(),
                    rects.len()
                ),
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
                crate::trace(&format!(
                    "run_probe_cycle: emitting selection:detected at ({x}, {y})"
                ));
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
        // Most classic controls expose the selected text on the focused element.
        if let Ok(element) = unsafe { automation.GetFocusedElement() } {
            match selection_from_element(&element) {
                Ok(Some(selection)) => return Ok(Some(selection)),
                Ok(None) => {}
                Err(e) => crate::trace(&format!("probe_selection: focused element error: {e:?}")),
            }
        }

        // Electron/web chat apps sometimes leave focus on a wrapper while the text
        // control under the mouse exposes TextPattern. Probe the mouse-up point as
        // a fallback so apps like Discord/WhatsApp/LINE get a second chance.
        let point = POINT {
            x: LAST_MOUSE_UP_X.load(Ordering::Relaxed),
            y: LAST_MOUSE_UP_Y.load(Ordering::Relaxed),
        };
        if point.x != 0 || point.y != 0 {
            if let Ok(element) = unsafe { automation.ElementFromPoint(point) } {
                return selection_from_element(&element);
            }
        }

        Ok(None)
    }

    /// Whether `element` is an editable text control, as opposed to read-only
    /// content (a web article, a PDF viewer page, chat message history, etc.)
    /// that merely happens to expose `TextPattern` so screen readers can read
    /// it. This is what confines the rewrite bubble to text fields — without
    /// it, any selectable text anywhere satisfies `probe_selection` below.
    ///
    /// `ValuePattern.CurrentIsReadOnly()` is the only signal used: live
    /// tracing against real content showed it's the one reliable indicator —
    /// editable surfaces (Notepad's edit control, browser address/search
    /// boxes, Chromium contenteditable) consistently report `IsReadOnly() ==
    /// false`, and a read-only Chromium document root correctly reports
    /// `true`. `TextEditPattern` availability looked like a plausible
    /// fallback for controls that don't expose `ValuePattern`, but tracing
    /// showed Chromium hands it out even on a plain read-only "Text"
    /// control-type paragraph (the exact article-reading case this is meant
    /// to exclude), so it's not used. If `ValuePattern` isn't present at all,
    /// the element is treated as not editable; this deliberately fails closed
    /// (no bubble) rather than open, since popping the bubble over read-only
    /// content is the exact bug being fixed here.
    fn is_element_editable(element: &IUIAutomationElement) -> bool {
        let Ok(value_pattern) =
            (unsafe { element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) })
        else {
            return false;
        };
        unsafe { value_pattern.CurrentIsReadOnly() }
            .map(|read_only| !read_only.as_bool())
            .unwrap_or(false)
    }

    fn selection_from_element(
        element: &IUIAutomationElement,
    ) -> WinResult<Option<(String, Vec<(f64, f64, f64, f64)>)>> {
        if !is_element_editable(element) {
            return Ok(None);
        }

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
}

#[cfg(target_os = "windows")]
pub use win::{focus_last_source_window, last_anchor, start, stop, AnchorPayload};

#[cfg(target_os = "macos")]
mod mac {
    use core::ffi::{c_char, c_double, c_float, c_void};
    use std::ffi::CString;
    use std::sync::{
        atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering},
        mpsc, Mutex, OnceLock,
    };
    use std::time::Duration;
    use tauri::{AppHandle, Emitter, Manager};

    // Coordinate-space research (2026-07-11, via Apple's own published docs and
    // corroborating third-party source): `AXUIElementCopyElementAtPosition` is
    // documented as taking "top-left relative screen coordinates" — the top-left
    // corner of the primary display is (0, 0), +x right, +y down. `CGEventTap`'s
    // `CGEventGetLocation` and `CGDisplayBounds` (used below and in `lib.rs`'s
    // macOS `clamp_rect_to_monitor`) are both documented Quartz/CoreGraphics
    // global-display-coordinate-space APIs, which is that exact same top-left-
    // origin space. `kAXBoundsForRangeParameterizedAttribute`'s returned `CGRect`
    // is likewise in this space (it is explicitly a *screen* rect, not a
    // window-local one). So AX bounds, CGEventTap mouse locations, and
    // CGDisplayBounds are all directly comparable/combinable with **no**
    // conversion — none of them use AppKit's `NSScreen`/`NSView`
    // bottom-left-origin "flipped" space. This matters concretely: the anchor
    // math below (`selection_anchor_from_rect`) and `lib.rs`'s monitor-clamping
    // math both add/compare these values directly.
    //
    // AX thread-affinity research: Apple's reference docs for
    // `AXUIElementCopyAttributeValue`/`AXUIElementCopyElementAtPosition` do not
    // document any main-thread or run-loop requirement (unlike `AXObserver`,
    // whose notification delivery is explicitly run-loop-based and normally
    // wired to the main run loop). Hammerspoon's `libaxuielement.m` — a mature,
    // widely used AX-heavy macOS tool — calls these same functions directly with
    // no `dispatch_async`/main-thread marshaling. Architecturally these are
    // synchronous Mach-IPC round trips to the target app's own accessibility
    // server, not calls into this process's AppKit/window-server state, which is
    // why they don't carry the same-thread requirement `NSWorkspace`/
    // `NSRunningApplication` calls do elsewhere in this codebase (see
    // `foreground.rs`, `lib.rs`'s paste-target functions). Given no documented
    // restriction and matching precedent, `probe_selection` below is called
    // directly from the worker thread, unmarshaled — consistent with how this
    // module already worked before this pass.

    const MIN_SELECTION_CHARS: usize = 2;
    const EVENT_LEFT_MOUSE_DOWN: CGEventType = 1;
    const EVENT_LEFT_MOUSE_UP: CGEventType = 2;
    const EVENT_KEY_DOWN: CGEventType = 10;
    const EVENT_KEY_UP: CGEventType = 11;
    const EVENT_FLAGS_CHANGED: CGEventType = 12;
    const EVENT_TAP_DISABLED_BY_TIMEOUT: CGEventType = 0xFFFF_FFFE;
    const EVENT_TAP_DISABLED_BY_USER_INPUT: CGEventType = 0xFFFF_FFFF;
    const KEY_ESCAPE: i64 = 53;
    const KEY_A: i64 = 0;
    const KEY_DELETE: i64 = 51;
    const KEY_FORWARD_DELETE: i64 = 117;
    const FIELD_KEYCODE: CGEventField = 9;
    const FLAG_MASK_COMMAND: i64 = 1 << 20;
    const FLAG_MASK_ALTERNATE: i64 = 1 << 19;
    const AX_ERROR_SUCCESS: AXError = 0;
    const AX_ERROR_ATTRIBUTE_UNSUPPORTED: AXError = -25205;
    const AX_VALUE_CGRECT: AXValueType = 3;
    const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const AX_FOCUSED_UI_ELEMENT: &str = "AXFocusedUIElement";
    const AX_SELECTED_TEXT: &str = "AXSelectedText";
    const AX_SELECTED_TEXT_RANGE: &str = "AXSelectedTextRange";
    const AX_BOUNDS_FOR_RANGE: &str = "AXBoundsForRange";
    const AX_VALUE: &str = "AXValue";
    /// See `is_element_editable`'s role-based branch for why this is read.
    const AX_ROLE: &str = "AXRole";
    /// See `maybe_activate_manual_accessibility` for why this is needed.
    const AX_MANUAL_ACCESSIBILITY: &str = "AXManualAccessibility";
    /// Chrome's application-level signal that a real assistive-technology
    /// client needs the complete web accessibility tree. See
    /// `maybe_activate_frontmost_application`.
    const AX_ENHANCED_USER_INTERFACE: &str = "AXEnhancedUserInterface";
    /// Current Chromium deliberately debounces `AXEnhancedUserInterface` for
    /// two seconds on macOS Sonoma+ before enabling complete accessibility,
    /// and Electron's `AXManualAccessibility` tree populates on its own
    /// asynchronous schedule too. A single fixed-delay re-probe either lands
    /// before the tree is ready (permanently missing that selection, since
    /// nothing else re-probes it) or wastes time waiting past when the tree
    /// was actually ready. Instead, re-probe several times at increasing
    /// delays — an early hit stops the schedule immediately at the next
    /// message drain (see `worker_loop`'s inner `recv_timeout` loop, which
    /// collapses redundant back-to-back `Recheck`s into a single probe), and
    /// a late-populating tree still gets caught by a later attempt. None of
    /// these sleeps happen on the event-tap or worker thread — see
    /// `schedule_activation_reprobe`.
    const APPLICATION_ACTIVATION_REPROBE_SCHEDULE: [Duration; 4] = [
        Duration::from_millis(150),
        Duration::from_millis(400),
        Duration::from_millis(900),
        Duration::from_millis(2000),
    ];

    type AXError = i32;
    type AXValueType = u32;
    type CFAllocatorRef = *const c_void;
    type CFIndex = isize;
    type CFRunLoopRef = *mut c_void;
    type CFRunLoopSourceRef = *mut c_void;
    type CFStringRef = *const c_void;
    type CFTypeRef = *const c_void;
    type CFMachPortRef = *mut c_void;
    type CGEventRef = *mut c_void;
    type CGEventTapProxy = *const c_void;
    type CGEventType = u32;
    type CGEventField = u32;
    type CGEventMask = u64;
    type AXUIElementRef = *const c_void;
    type AXValueRef = *const c_void;
    type Boolean = u8;

    type CGEventTapCallBack = unsafe extern "C" fn(
        proxy: CGEventTapProxy,
        event_type: CGEventType,
        event: CGEventRef,
        user_info: *mut c_void,
    ) -> CGEventRef;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGPoint {
        x: c_double,
        y: c_double,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGSize {
        width: c_double,
        height: c_double,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    #[derive(Clone, Copy)]
    enum WorkerMsg {
        Recheck,
        Clear,
        Shutdown,
    }

    #[derive(Clone, serde::Serialize, serde::Deserialize)]
    pub struct AnchorPayload {
        pub x: f64,
        pub y: f64,
        pub text: String,
    }

    /// Lifecycle state for the event tap, serialized behind one mutex so
    /// start()/stop() (and a fast stop()-then-start() toggle, e.g. flipping
    /// the "Selection bubble" Settings switch off/on, or a fast Accessibility
    /// revoke→grant cycle) can't race each other. Ported verbatim from
    /// `esc_hook.rs`'s `mod mac` — that module hit and fixed this exact race
    /// class twice already (see its "critical-review fixes" doc comments):
    /// a bare pair of atomics (as this module used before this pass) allows
    /// (1) a lost-stop revival, where a dying old tap thread observes a new
    /// start()'s fresh `RUNNING=true`/`STOP_REQUESTED=false` and keeps
    /// running alongside the new one (two live system-wide taps, duplicate
    /// probing, a leaked thread), and (2) a stale-generation stomp, where an
    /// old thread's unconditional teardown (`RUNNING.store(false)`, handle
    /// clears) clobbers a newer generation's already-published state,
    /// silently killing the bubble for the rest of the session. Don't
    /// re-derive a third variant of this fix — reuse this design.
    enum TapState {
        /// Nothing installed, no thread in flight.
        Idle,
        /// Thread spawned, but it hasn't published its `CFRunLoopRef` yet.
        Starting,
        /// Tap installed and its run loop is live; holds the `CFRunLoopRef`
        /// (as a `usize` — see `esc_hook.rs`'s identical field for why this
        /// is sound: `CFRunLoopStop` is documented safe to call from any
        /// thread on a run loop object obtained from another thread).
        Running(usize),
    }

    static TAP: Mutex<TapState> = Mutex::new(TapState::Idle);
    static APP: OnceLock<AppHandle> = OnceLock::new();
    static NOTIFY_TX: Mutex<Option<mpsc::Sender<WorkerMsg>>> = Mutex::new(None);
    static TAP_REF: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
    /// Set by `stop()`, cleared by `run_tap_thread` once it has a live tap —
    /// same role and same "reset in the same critical section as publishing
    /// `Running`" discipline as `esc_hook.rs`'s `STOP_REQUESTED`. This is the
    /// *reliable* shutdown signal; `CFRunLoopStop` (also called by `stop()`)
    /// is just a best-effort fast path on top of it.
    static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
    static HAD_SELECTION: AtomicBool = AtomicBool::new(false);
    static LAST_ANCHOR: Mutex<Option<AnchorPayload>> = Mutex::new(None);
    /// Last left-mouse-up point (Quartz global screen coordinates — see
    /// coordinate-space research note above), stored as `f64::to_bits()` since
    /// there's no stable `AtomicF64`. Used as the fallback hit-test point for
    /// `AXUIElementCopyElementAtPosition` when the focused element has no
    /// usable selection (mirrors the Windows backend's `ElementFromPoint`
    /// fallback for Electron/web apps that leave focus on a wrapper element).
    static LAST_MOUSE_UP_X: AtomicU64 = AtomicU64::new(0);
    static LAST_MOUSE_UP_Y: AtomicU64 = AtomicU64::new(0);
    /// Mouse-down point, used only to compute drag distance on the following
    /// mouse-up (mirrors Windows' `mouse_hook_proc` — see `tap_callback`).
    static LAST_MOUSE_DOWN_X: AtomicU64 = AtomicU64::new(0);
    static LAST_MOUSE_DOWN_Y: AtomicU64 = AtomicU64::new(0);

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateSystemWide() -> AXUIElementRef;
        fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> AXError;
        fn AXUIElementCopyParameterizedAttributeValue(
            element: AXUIElementRef,
            parameterized_attribute: CFStringRef,
            parameter: CFTypeRef,
            value: *mut CFTypeRef,
        ) -> AXError;
        fn AXUIElementIsAttributeSettable(
            element: AXUIElementRef,
            attribute: CFStringRef,
            settable: *mut Boolean,
        ) -> AXError;
        // Signature confirmed via Apple's published reference docs (see
        // coordinate-space research note above `use` block): takes 32-bit
        // `float`, not `double`/`CGFloat`, in top-left-origin global screen
        // coordinates — the same space `CGEventGetLocation` reports mouse clicks
        // in, so the fallback below can pass `LAST_MOUSE_UP_*` straight through.
        fn AXUIElementCopyElementAtPosition(
            application: AXUIElementRef,
            x: c_float,
            y: c_float,
            element: *mut AXUIElementRef,
        ) -> AXError;
        fn AXValueGetValue(
            value: AXValueRef,
            the_type: AXValueType,
            value_ptr: *mut c_void,
        ) -> Boolean;
        fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> AXError;
        fn AXUIElementSetAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: CFTypeRef,
        ) -> AXError;

    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFRunLoopDefaultMode: CFStringRef;
        static kCFBooleanFalse: CFTypeRef;
        static kCFBooleanTrue: CFTypeRef;
        fn CFStringCreateWithCString(
            alloc: CFAllocatorRef,
            c_str: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFRelease(cf: CFTypeRef);
        fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
        fn CFStringGetLength(the_string: CFStringRef) -> CFIndex;
        fn CFStringGetMaximumSizeForEncoding(length: CFIndex, encoding: u32) -> CFIndex;
        fn CFStringGetCString(
            the_string: CFStringRef,
            buffer: *mut c_char,
            buffer_size: CFIndex,
            encoding: u32,
        ) -> Boolean;
        fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        fn CFRunLoopRunInMode(
            mode: CFStringRef,
            seconds: c_double,
            return_after_source_handled: bool,
        ) -> i32;
        fn CFRunLoopStop(rl: CFRunLoopRef);
        fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
        fn CFRunLoopRemoveSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
        fn CFMachPortInvalidate(port: CFMachPortRef);
        fn CFMachPortCreateRunLoopSource(
            allocator: CFAllocatorRef,
            port: CFMachPortRef,
            order: CFIndex,
        ) -> CFRunLoopSourceRef;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventTapCreate(
            tap: u32,
            place: u32,
            options: u32,
            events_of_interest: CGEventMask,
            callback: CGEventTapCallBack,
            user_info: *mut c_void,
        ) -> CFMachPortRef;
        fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
        fn CGEventGetIntegerValueField(event: CGEventRef, field: CGEventField) -> i64;
        fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
        // Modifier flags are NOT a `CGEventField` readable via
        // `CGEventGetIntegerValueField` — there is no such enumerator in
        // Apple's `CGEventTypes.h` (field 12 is
        // `kCGScrollWheelEventDeltaAxis2`, unrelated). Flags are their own
        // dedicated accessor returning `CGEventFlags` (a `u64` bitmask).
        // Reading field 12 on a keyDown always returned 0, which silently
        // broke Cmd+A detection (the mask compare was always false) — fixed
        // by calling this instead.
        fn CGEventGetFlags(event: CGEventRef) -> u64;
    }

    pub fn last_anchor() -> Option<AnchorPayload> {
        LAST_ANCHOR.lock().unwrap().clone()
    }

    pub fn focus_last_source_window() {
        if let Some(app) = APP.get() {
            crate::focus_paste_target_window(app);
        } else {
            crate::trace("selection_watcher::focus_last_source_window(mac): app handle missing");
        }
    }

    /// Install the tap and start the worker if not already running (or
    /// already attempting to start). Idempotent — see `TapState`'s doc
    /// comment for why this is a `Mutex`-serialized state machine rather than
    /// a bare atomic. The fresh `mpsc` channel/worker thread are only created
    /// AFTER this call has won the exclusive right to start (state moved
    /// `Idle` -> `Starting` under the same lock), so a racing concurrent
    /// `start()` can never spawn a second, orphaned worker.
    pub fn start(app: &AppHandle) {
        let _ = APP.get_or_init(|| app.clone());

        {
            let mut state = TAP.lock().unwrap();
            if !matches!(*state, TapState::Idle) {
                crate::trace("selection_watcher::start(mac): already running");
                return;
            }
            if !crate::clipboard::accessibility_trusted(false) {
                crate::trace(
                    "selection_watcher::start(mac): Accessibility not trusted; watcher skipped",
                );
                return; // stays Idle; next start() call (e.g. after permission
                        // is granted) will re-check.
            }
            *state = TapState::Starting;
        }

        let (tx, rx) = mpsc::channel();
        *NOTIFY_TX.lock().unwrap() = Some(tx);
        std::thread::spawn(move || worker_loop(rx));

        crate::trace("selection_watcher::start(mac): spawning tap thread");
        std::thread::spawn(run_tap_thread);
    }

    /// Uninstall the tap and stop the worker. Safe to call even if nothing is
    /// running or the tap is still mid-startup. Mirrors `esc_hook.rs`'s
    /// `stop()`: `STOP_REQUESTED` and the `TapState` transition happen in the
    /// same critical section (guarded by `TAP`'s lock), so this can never
    /// race `run_tap_thread` publishing `Running` and clearing
    /// `STOP_REQUESTED` — whichever happens first runs to completion before
    /// the other can observe the lock.
    pub fn stop() {
        let was_active;
        {
            let mut state = TAP.lock().unwrap();
            was_active = !matches!(*state, TapState::Idle);
            // Also gates any worker probe that was already in flight from
            // activating a target application after teardown has begun.
            STOP_REQUESTED.store(true, Ordering::SeqCst);
            match *state {
                TapState::Idle => {}
                TapState::Starting => {
                    *state = TapState::Idle;
                }
                TapState::Running(rl_ptr) => {
                    unsafe { CFRunLoopStop(rl_ptr as CFRunLoopRef) };
                    *state = TapState::Idle;
                }
            }
        }
        if let Some(tx) = NOTIFY_TX.lock().unwrap().take() {
            let _ = tx.send(WorkerMsg::Shutdown);
        }
        unsafe { deactivate_frontmost_applications() };
        if !was_active {
            return;
        }
        clear_selection();
    }

    fn run_tap_thread() {
        unsafe {
            let mask = (1_u64 << EVENT_LEFT_MOUSE_DOWN)
                | (1_u64 << EVENT_LEFT_MOUSE_UP)
                | (1_u64 << EVENT_KEY_DOWN)
                | (1_u64 << EVENT_KEY_UP)
                | (1_u64 << EVENT_FLAGS_CHANGED);
            let tap = CGEventTapCreate(0, 0, 1, mask, tap_callback, std::ptr::null_mut());
            if tap.is_null() {
                // Most likely Accessibility was revoked between start()'s
                // check and here. Reset to Idle so a later start() (e.g.
                // after permission is re-granted) can try again.
                crate::trace(
                    "selection_watcher::run_tap_thread(mac): CGEventTapCreate failed, leaving Idle",
                );
                *TAP.lock().unwrap() = TapState::Idle;
                return;
            }

            // Publish the real handle before anything can start relying on
            // it (the tap isn't enabled yet, so the callback can't fire
            // before this point) — mirrors esc_hook.rs's TAP_REF_SLOT.
            TAP_REF.store(tap, Ordering::SeqCst);

            let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
            if source.is_null() {
                TAP_REF.store(std::ptr::null_mut(), Ordering::SeqCst);
                CFMachPortInvalidate(tap);
                CFRelease(tap as CFTypeRef);
                crate::trace(
                    "selection_watcher::run_tap_thread(mac): CFMachPortCreateRunLoopSource failed, leaving Idle",
                );
                *TAP.lock().unwrap() = TapState::Idle;
                return;
            }

            let rl = CFRunLoopGetCurrent();

            // Publish the run loop handle so stop() can signal us — but only
            // if nobody called stop() while we were still starting up.
            {
                let mut state = TAP.lock().unwrap();
                if !matches!(*state, TapState::Starting) {
                    crate::trace(
                        "selection_watcher::run_tap_thread(mac): stop() ran during startup, tearing down without entering the run loop",
                    );
                    drop(state);
                    TAP_REF.store(std::ptr::null_mut(), Ordering::SeqCst);
                    CFMachPortInvalidate(tap);
                    CFRelease(source as CFTypeRef);
                    CFRelease(tap as CFTypeRef);
                    return;
                }
                *state = TapState::Running(rl as usize);
                // Same critical section as the state write above, so this can
                // never race a concurrent stop() setting it true.
                STOP_REQUESTED.store(false, Ordering::SeqCst);
            }

            CFRunLoopAddSource(rl, source, kCFRunLoopDefaultMode);
            CGEventTapEnable(tap, true);
            crate::trace("selection_watcher::run_tap_thread(mac): tap running");

            // Bounded poll instead of a single blocking run — see
            // esc_hook.rs's identical loop for the full race rationale.
            // Re-checking STOP_REQUESTED every 0.25s guarantees we notice a
            // stop within one interval regardless of whether CFRunLoopStop's
            // signal landed; the secondary generation check below is what
            // actually prevents the "lost-stop revival" failure mode (a
            // fresh start() resetting STOP_REQUESTED to false out from under
            // this exact generation) — if state no longer reflects OUR run
            // loop, some newer generation owns it and we must stop too.
            loop {
                let _ = CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.25, false);
                check_frontmost_app_switch();
                if STOP_REQUESTED.load(Ordering::SeqCst) {
                    break;
                }
                let state = TAP.lock().unwrap();
                if !matches!(*state, TapState::Running(ptr) if ptr == rl as usize) {
                    break;
                }
            }

            crate::trace("selection_watcher::run_tap_thread(mac): tap stopping");
            CGEventTapEnable(tap, false);
            CFRunLoopRemoveSource(rl, source, kCFRunLoopDefaultMode);
            CFMachPortInvalidate(tap);
            CFRelease(source as CFTypeRef);
            CFRelease(tap as CFTypeRef);
            TAP_REF.store(std::ptr::null_mut(), Ordering::SeqCst);

            // Only clear state if it's still ours to clear — the same
            // generation guard esc_hook.rs uses to avoid a stale-generation
            // stomp on a newer thread's already-published Running state.
            let mut state = TAP.lock().unwrap();
            if matches!(*state, TapState::Running(ptr) if ptr == rl as usize) {
                *state = TapState::Idle;
            }
        }
    }

    unsafe extern "C" fn tap_callback(
        _proxy: CGEventTapProxy,
        event_type: CGEventType,
        event: CGEventRef,
        _user_info: *mut c_void,
    ) -> CGEventRef {
        if event_type == EVENT_TAP_DISABLED_BY_TIMEOUT
            || event_type == EVENT_TAP_DISABLED_BY_USER_INPUT
        {
            crate::trace("selection_watcher::tap_callback(mac): tap disabled by OS, re-enabling");
            let tap = TAP_REF.load(Ordering::SeqCst);
            if !tap.is_null() {
                unsafe { CGEventTapEnable(tap, true) };
            }
            return event;
        }

        // Mouse-down needs no worker handoff at all (mirrors Windows'
        // `mouse_hook_proc`, which just records the down point in an atomic) —
        // handle it before the `NOTIFY_TX` lookup below so a `start()` that
        // hasn't finished wiring the channel yet still gets the down point
        // recorded correctly for the very first mouse-up.
        if event_type == EVENT_LEFT_MOUSE_DOWN {
            let loc = unsafe { CGEventGetLocation(event) };
            LAST_MOUSE_DOWN_X.store(loc.x.to_bits(), Ordering::Relaxed);
            LAST_MOUSE_DOWN_Y.store(loc.y.to_bits(), Ordering::Relaxed);
            return event;
        }

        let Some(tx) = NOTIFY_TX.lock().unwrap().as_ref().cloned() else {
            return event;
        };

        match event_type {
            EVENT_LEFT_MOUSE_UP => {
                let loc = unsafe { CGEventGetLocation(event) };
                LAST_MOUSE_UP_X.store(loc.x.to_bits(), Ordering::Relaxed);
                LAST_MOUSE_UP_Y.store(loc.y.to_bits(), Ordering::Relaxed);

                // Drag-distance check mirrors Windows' `mouse_hook_proc`
                // `dx.abs() > 8 || dy.abs() > 8` threshold, but note the
                // *use* here: Windows passes `was_drag` straight through as
                // `maybe_close_bubble_menu_on_outside_click`'s
                // `allow_probe_after_close` argument, where `true` (a real
                // drag, i.e. plausibly a fresh text selection made elsewhere)
                // means "don't suppress the next probe" — the opposite of
                // what the name reads as at a glance for a plain click. This
                // mirrors that exact intent, not just the shape: a real drag
                // outside the menu should let the worker probe the freshly
                // dragged selection immediately, while a simple click outside
                // (no drag) suppresses the immediately-following probe for a
                // short window, same as every other menu-close path.
                let down_x = f64::from_bits(LAST_MOUSE_DOWN_X.load(Ordering::Relaxed));
                let down_y = f64::from_bits(LAST_MOUSE_DOWN_Y.load(Ordering::Relaxed));
                let dx = loc.x - down_x;
                let dy = loc.y - down_y;
                let was_drag = dx.abs() > 8.0 || dy.abs() > 8.0;
                if let Some(app) = APP.get() {
                    crate::maybe_close_bubble_menu_on_outside_click(
                        app,
                        loc.x as i32,
                        loc.y as i32,
                        was_drag,
                    );
                }

                let _ = tx.send(WorkerMsg::Recheck);
            }
            EVENT_KEY_DOWN => {
                let key = unsafe { CGEventGetIntegerValueField(event, FIELD_KEYCODE) };
                let flags = unsafe { CGEventGetFlags(event) } as i64;
                let cmd_a = key == KEY_A
                    && (flags & FLAG_MASK_COMMAND) != 0
                    && (flags & FLAG_MASK_ALTERNATE) == 0;
                let edit_key = key == KEY_DELETE || key == KEY_FORWARD_DELETE;
                if cmd_a || (edit_key && HAD_SELECTION.load(Ordering::Relaxed)) {
                    let _ = tx.send(WorkerMsg::Recheck);
                } else if key == KEY_ESCAPE && HAD_SELECTION.load(Ordering::Relaxed) {
                    let _ = tx.send(WorkerMsg::Clear);
                }
            }
            EVENT_KEY_UP | EVENT_FLAGS_CHANGED => {
                if HAD_SELECTION.load(Ordering::Relaxed) {
                    let _ = tx.send(WorkerMsg::Recheck);
                }
            }
            _ => {}
        }

        event
    }

    fn worker_loop(rx: mpsc::Receiver<WorkerMsg>) {
        'outer: while let Ok(msg) = rx.recv() {
            match msg {
                WorkerMsg::Shutdown => break,
                WorkerMsg::Clear => {
                    clear_selection();
                    continue;
                }
                WorkerMsg::Recheck => {}
            }

            loop {
                match rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(WorkerMsg::Recheck) => continue,
                    Ok(WorkerMsg::Clear) => {
                        clear_selection();
                        continue 'outer;
                    }
                    Ok(WorkerMsg::Shutdown) => break 'outer,
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break 'outer,
                }
            }

            run_probe_cycle();
        }
        clear_selection();
    }

    fn run_probe_cycle() {
        if bubble_menu_is_open() {
            crate::trace("selection_watcher::run_probe_cycle(mac): skipped, bubble_menu open/recently closed");
            return;
        }

        let probe_result = unsafe { probe_selection() };
        crate::trace(&format!(
            "selection_watcher::run_probe_cycle(mac): probe_selection -> {}",
            match &probe_result {
                Ok(Some((text, rect))) => format!(
                    "Some({} chars, rect {},{} {}x{})",
                    text.trim().chars().count(),
                    rect.origin.x,
                    rect.origin.y,
                    rect.size.width,
                    rect.size.height
                ),
                Ok(None) => "None".to_string(),
                Err(e) => format!("Err({e})"),
            }
        ));

        match probe_result.unwrap_or(None) {
            Some((text, rect)) if is_selection_significant(&text) => {
                HAD_SELECTION.store(true, Ordering::Relaxed);
                let (x, y) = selection_anchor_from_rect(rect);
                let payload = AnchorPayload { x, y, text };
                *LAST_ANCHOR.lock().unwrap() = Some(payload.clone());
                remember_source_pid();
                if let Some(app) = APP.get() {
                    let _ = app.emit("selection:detected", payload);
                }
            }
            _ => clear_selection(),
        }
    }

    /// Same "no selection" filter as the Windows backend's `MIN_SELECTION_CHARS`
    /// gate: trims whitespace first so a selection that's only whitespace or a
    /// single incidental character grabbed mid-click doesn't pop the bubble.
    /// Pure/no-FFI so it's directly unit-testable — see `mod tests` below.
    fn is_selection_significant(text: &str) -> bool {
        text.trim().chars().count() >= MIN_SELECTION_CHARS
    }

    /// Anchor = bottom-right corner of the AX selection bounds rect, matching
    /// the Windows backend's "end of the last bounding rect" convention.
    /// Pure/no-FFI (takes/returns plain structs) so it's directly
    /// unit-testable — see `mod tests` below.
    fn selection_anchor_from_rect(rect: CGRect) -> (f64, f64) {
        (
            rect.origin.x + rect.size.width,
            rect.origin.y + rect.size.height,
        )
    }

    fn clear_selection() {
        let had_selection = HAD_SELECTION.swap(false, Ordering::Relaxed);
        *LAST_ANCHOR.lock().unwrap() = None;
        if had_selection {
            crate::trace("selection_watcher::clear_selection(mac): emitting selection:cleared");
            if let Some(app) = APP.get() {
                let _ = app.emit("selection:cleared", ());
            }
        }
    }

    /// Dedicated frontmost-application-change check, polled once per
    /// `run_tap_thread` iteration (every ~0.25s — see the loop above). This
    /// exists because switching *which app is frontmost* — Cmd+Tab, a Dock
    /// click, a trackpad swipe, Mission Control — does not reliably generate
    /// any CGEvent in this tap's mouse/keyboard mask (unlike, say, clicking
    /// into an already-frontmost other app, which the existing mouse-up
    /// probe in `tap_callback` already handles). Without this, the bubble
    /// stays stuck on screen, anchored to a selection in an app that's no
    /// longer frontmost, until the user happens to click or type again.
    /// Cheap no-op when there's no bubble tracked.
    fn check_frontmost_app_switch() {
        if !HAD_SELECTION.load(Ordering::Relaxed) {
            return;
        }
        let Some(app) = APP.get() else {
            return;
        };
        // Fire-and-forget: unlike `foreground::detect_impl`'s synchronous
        // main-thread round trip (fine there — it's a one-shot call kicked
        // off by a hotkey), this runs every ~0.25s on the same thread that
        // owns the CGEventTap's run loop, so it must never block waiting on
        // a reply. The closure does its own comparison and sends
        // `WorkerMsg::Clear` itself instead of reporting back.
        let _ = app.run_on_main_thread(|| {
            use objc2_app_kit::{NSRunningApplication, NSWorkspace};
            let frontmost_pid = unsafe {
                NSWorkspace::sharedWorkspace()
                    .frontmostApplication()
                    .map(|running_app| running_app.processIdentifier())
            };
            let own_pid = unsafe { NSRunningApplication::currentApplication().processIdentifier() };
            // The source app (the one that owned the selection when it was
            // detected — see `remember_source_pid`/`crate::paste_target_pid`)
            // is deliberately still frontmost for the whole time the bubble
            // is visible: `show_bubble`/`show_bubble_menu` build those
            // windows with `.focused(false)` and never call `.set_focus()`,
            // specifically so showing the bubble doesn't steal foreground
            // from the source app. So the source app being frontmost is the
            // normal steady state, not a switch — only clear when frontmost
            // is neither reWrite itself (legitimate: user clicked the
            // bubble/menu) nor the source app.
            let source_pid = crate::paste_target_pid();
            if should_clear_for_frontmost_switch(own_pid, source_pid, frontmost_pid) {
                if let Some(tx) = NOTIFY_TX.lock().unwrap().as_ref().cloned() {
                    let _ = tx.send(WorkerMsg::Clear);
                }
            }
        });
    }

    /// Pure decision logic behind `check_frontmost_app_switch`, split out so
    /// it's directly unit-testable without any AppKit FFI — see `mod tests`
    /// below. `None` (no frontmost app reported) is treated as "don't clear"
    /// rather than "clear": a transient `NSWorkspace` lookup failure/gap
    /// shouldn't hide a bubble that may still be pointing at the right app.
    fn should_clear_for_frontmost_switch(
        own_pid: i32,
        source_pid: i32,
        frontmost_pid: Option<i32>,
    ) -> bool {
        match frontmost_pid {
            Some(pid) => pid != own_pid && pid != source_pid,
            None => false,
        }
    }

    fn bubble_menu_is_open() -> bool {
        crate::bubble_menu_probe_suppressed()
            || APP
                .get()
                .and_then(|app| app.get_webview_window("bubble_menu"))
                .and_then(|w| w.outer_position().ok())
                .map(|pos| !crate::is_bubble_menu_parked(pos.x, pos.y))
                .unwrap_or(false)
    }

    fn remember_source_pid() {
        let Some(app) = APP.get() else {
            return;
        };
        crate::remember_paste_target_window(app);
    }

    /// Read the frontmost application's *main* pid and bundle id on AppKit's
    /// main thread.
    /// This deliberately does not use `AXUIElementGetPid` on a web node:
    /// Chromium web elements can belong to renderer-side accessibility
    /// objects, while the activation attributes below are implemented by the
    /// main browser application's `BrowserCrApplication` object.
    struct FrontmostApplication {
        pid: i32,
        bundle_id: Option<String>,
    }

    fn frontmost_application() -> Option<FrontmostApplication> {
        let app = APP.get()?;
        let (tx, rx) = mpsc::channel();
        let posted = app.run_on_main_thread(move || {
            use objc2_app_kit::NSWorkspace;
            let application = unsafe {
                NSWorkspace::sharedWorkspace()
                    .frontmostApplication()
                    .map(|running_app| FrontmostApplication {
                        pid: running_app.processIdentifier(),
                        bundle_id: running_app
                            .bundleIdentifier()
                            .map(|bundle_id| bundle_id.to_string()),
                    })
            };
            let _ = tx.send(application);
        });
        if posted.is_err() {
            return None;
        }
        rx.recv_timeout(Duration::from_millis(500)).ok().flatten()
    }

    unsafe fn probe_selection() -> Result<Option<(String, CGRect)>, String> {
        if !crate::clipboard::accessibility_trusted(false) {
            return Ok(None);
        }

        let system = AXUIElementCreateSystemWide();
        if system.is_null() {
            return Ok(None);
        }

        // Most classic/native controls expose the selection on the focused
        // element — try that first, mirroring the Windows backend's
        // `GetFocusedElement()` pass.
        let mut focused: CFTypeRef = std::ptr::null();
        let attr = cfstring(AX_FOCUSED_UI_ELEMENT)?;
        let err = AXUIElementCopyAttributeValue(system, attr, &mut focused);
        CFRelease(attr as CFTypeRef);
        if err == AX_ERROR_SUCCESS && !focused.is_null() {
            let result = selection_from_element(focused as AXUIElementRef);
            if !matches!(result, Ok(Some(_))) {
                maybe_activate_manual_accessibility(focused as AXUIElementRef);
            }
            CFRelease(focused);
            match result {
                Ok(Some(selection)) => {
                    CFRelease(system as CFTypeRef);
                    return Ok(Some(selection));
                }
                Ok(None) => {}
                Err(e) => crate::trace(&format!(
                    "selection_watcher::probe_selection(mac): focused element error: {e}"
                )),
            }
        }

        // Electron/web-app fallback: some apps leave focus on a wrapper element
        // while the actual text control under the cursor exposes the selection.
        // Hit-test the last mouse-up point instead, mirroring the Windows
        // backend's `ElementFromPoint` fallback for the same class of apps
        // (Discord/WhatsApp/Slack/Notion-style Electron UIs).
        let x = f64::from_bits(LAST_MOUSE_UP_X.load(Ordering::Relaxed)) as c_float;
        let y = f64::from_bits(LAST_MOUSE_UP_Y.load(Ordering::Relaxed)) as c_float;
        let point_result = if x != 0.0 || y != 0.0 {
            let mut at_point: AXUIElementRef = std::ptr::null();
            let err = AXUIElementCopyElementAtPosition(system, x, y, &mut at_point);
            if err == AX_ERROR_SUCCESS && !at_point.is_null() {
                let r = selection_from_element(at_point);
                if !matches!(r, Ok(Some(_))) {
                    maybe_activate_manual_accessibility(at_point);
                }
                CFRelease(at_point);
                r
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        };
        CFRelease(system as CFTypeRef);
        match point_result {
            Ok(Some(selection)) => return Ok(Some(selection)),
            Ok(None) => {}
            Err(e) => crate::trace(&format!(
                "selection_watcher::probe_selection(mac): system hit-test element error: {e}"
            )),
        }

        // Chrome web content has a bootstrapping problem when accessibility is
        // initially disabled: the system-wide focused-element lookup is empty,
        // while the system-wide point hit-test returns only Chrome's top-level
        // AXScrollArea. That container is correctly rejected by the editable
        // gate above and rejects AXManualAccessibility, so neither path can
        // discover the selected <input>/<textarea>/contenteditable descendant.
        //
        // Resolve Chrome's *main* application pid and bundle id through
        // NSWorkspace, safely activate that application object (manual first;
        // enhanced only for recognized Chromium browsers), then query focus and
        // point-hit-test relative to that application.
        // Every element still goes through `selection_from_element`, including
        // `is_element_editable`, so this new route cannot make read-only page
        // paragraphs eligible for the rewrite bubble.
        let Some(frontmost) = frontmost_application() else {
            return Ok(None);
        };
        let application = AXUIElementCreateApplication(frontmost.pid);
        if application.is_null() {
            return Ok(None);
        }

        let activated = maybe_activate_frontmost_application(
            application,
            frontmost.pid,
            frontmost.bundle_id.as_deref(),
        );
        if activated {
            schedule_activation_reprobe();
        }

        let mut app_focused: CFTypeRef = std::ptr::null();
        let focused_attr = match cfstring(AX_FOCUSED_UI_ELEMENT) {
            Ok(attr) => attr,
            Err(error) => {
                CFRelease(application as CFTypeRef);
                return Err(error);
            }
        };
        let focused_err =
            AXUIElementCopyAttributeValue(application, focused_attr, &mut app_focused);
        CFRelease(focused_attr as CFTypeRef);
        if focused_err == AX_ERROR_SUCCESS && !app_focused.is_null() {
            let result = selection_from_element(app_focused as AXUIElementRef);
            CFRelease(app_focused);
            match result {
                Ok(Some(selection)) => {
                    CFRelease(application as CFTypeRef);
                    return Ok(Some(selection));
                }
                Ok(None) => {}
                Err(e) => crate::trace(&format!(
                    "selection_watcher::probe_selection(mac): app focused element error: {e}"
                )),
            }
        }

        let result = if x != 0.0 || y != 0.0 {
            let mut at_point: AXUIElementRef = std::ptr::null();
            let err = AXUIElementCopyElementAtPosition(application, x, y, &mut at_point);
            if err == AX_ERROR_SUCCESS && !at_point.is_null() {
                let result = selection_from_element(at_point);
                CFRelease(at_point as CFTypeRef);
                result
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        };
        CFRelease(application as CFTypeRef);
        result
    }

    /// Whether `role` (an `AXRole` string) names a control that's always a
    /// text-editable field. Pure/no-FFI so it's directly unit-testable — see
    /// `mod tests` below. This is a narrow allowlist, not a guess: these are
    /// the roles Chromium/WebKit use for `<input>` (`AXTextField`),
    /// `<textarea>`/contenteditable (`AXTextArea`), `<input type=search>`
    /// (`AXSearchField`), and autocomplete/address-bar-style combo inputs
    /// (`AXComboBox`). Deliberately excludes roles like `AXStaticText` and
    /// `AXWebArea`/`AXGroup` that cover read-only article/page content —
    /// widening this list is how the read-only exclusion would quietly break,
    /// so any addition needs the same "is this always user-editable text" bar.
    fn is_editable_role(role: &str) -> bool {
        matches!(
            role,
            "AXTextField" | "AXTextArea" | "AXComboBox" | "AXSearchField"
        )
    }

    /// Whether `element` is an editable text control, mirroring the Windows
    /// backend's `is_element_editable` (see that function's doc comment for
    /// why this gate exists — it's what confines the rewrite bubble to text
    /// fields instead of any selectable text, e.g. a Safari article). AX has
    /// no single "read-only" flag equivalent to UIA's `ValuePattern`, but
    /// `AXUIElementIsAttributeSettable` on `AXValue` is the direct analog:
    /// true only for controls whose value/text can actually be edited.
    ///
    /// That settable check alone isn't enough, though: Chromium/WebKit browser
    /// editable fields (Gmail compose, search boxes, plain `<textarea>`/
    /// `<input>`) frequently report `AXValue` as NOT settable even while
    /// genuinely editable, because their AX tree is populated lazily and the
    /// settable bit lags behind. So this also accepts the element via its
    /// `AXRole` — see `is_editable_role` — as a second, independent path to
    /// "editable". If the role can't be read, it falls back to the settable
    /// check alone. Fails closed (not editable) on any AX error, same rationale
    /// as the Windows side — read-only content must never win a probe error's
    /// benefit of the doubt.
    unsafe fn is_element_editable(element: AXUIElementRef) -> bool {
        if let Ok(attr) = cfstring(AX_VALUE) {
            let mut settable: Boolean = 0;
            let err = AXUIElementIsAttributeSettable(element, attr, &mut settable);
            CFRelease(attr as CFTypeRef);
            if err == AX_ERROR_SUCCESS && settable != 0 {
                return true;
            }
        }

        let Ok(role_attr) = cfstring(AX_ROLE) else {
            return false;
        };
        let mut role_value: CFTypeRef = std::ptr::null();
        let err = AXUIElementCopyAttributeValue(element, role_attr, &mut role_value);
        CFRelease(role_attr as CFTypeRef);
        if err != AX_ERROR_SUCCESS || role_value.is_null() {
            return false;
        }
        let role = cfstring_to_string(role_value as CFStringRef);
        CFRelease(role_value);
        match role {
            Ok(role) => is_editable_role(&role),
            Err(_) => false,
        }
    }

    #[derive(Clone, Copy)]
    enum ApplicationActivationKind {
        EnhancedUserInterface,
        ManualAccessibility,
        Unsupported,
    }

    #[derive(Clone, Copy)]
    struct ApplicationActivation {
        pid: i32,
        kind: ApplicationActivationKind,
        attempts: u32,
    }

    /// One entry per frontmost main application pid attempted during this
    /// watcher lifetime. Successful entries are balanced back to `false` by
    /// `deactivate_frontmost_applications` on stop; unsupported entries are
    /// retried up to `MAX_ACTIVATION_ATTEMPTS_PER_PID` times (see
    /// `should_attempt_activation`) rather than being retried forever or
    /// locked out after one try.
    static AX_APPLICATION_ACTIVATIONS: Mutex<Vec<ApplicationActivation>> = Mutex::new(Vec::new());

    /// Small bound on redundant activation attempts per pid, shared by both
    /// `AX_APPLICATION_ACTIVATIONS` and `AX_ACTIVATED_PIDS`. A single failed
    /// attempt — e.g. one made while the target process's AX bridge was
    /// still initializing — previously left that pid locked out for the rest
    /// of the watcher's lifetime, with no way to recover even though a later
    /// attempt against the same still-running process might well succeed.
    /// Capped, rather than unbounded, so a process that genuinely never
    /// supports either attribute doesn't get probed on every single
    /// selection for its whole lifetime.
    const MAX_ACTIVATION_ATTEMPTS_PER_PID: u32 = 3;

    /// Pure decision behind the retry cap above: whether a pid already
    /// attempted `attempts` times may be attempted again. Split out so it's
    /// directly unit-testable — see `mod tests` below.
    fn should_attempt_activation(attempts: u32) -> bool {
        attempts < MAX_ACTIVATION_ATTEMPTS_PER_PID
    }

    /// Chromium browser application bundle ids allowed to receive the strong
    /// `AXEnhancedUserInterface` signal. Electron apps deliberately are not in
    /// this list: their supported activation route is AXManualAccessibility.
    /// Normalize case so the safety decision does not depend on conventional
    /// capitalization differences between browser channels.
    fn is_chromium_browser_bundle_id(bundle_id: &str) -> bool {
        let bundle_id = bundle_id.to_ascii_lowercase();
        [
            "com.google.chrome",
            "org.chromium.chromium",
            "com.microsoft.edgemac",
            "com.brave.browser",
            "company.thebrowser.browser",
            "com.vivaldi.vivaldi",
            "com.operasoftware.opera",
            "com.operasoftware.operagx",
        ]
        .iter()
        .any(|base| {
            bundle_id == *base
                || bundle_id
                    .strip_prefix(base)
                    .is_some_and(|suffix| suffix.starts_with('.'))
        })
    }

    /// Activate the accessibility tree through the target application's own
    /// AX object. First try `AXManualAccessibility`, the narrow activation
    /// signal officially supported by Electron. Only when the target rejects
    /// that specific attribute as unsupported *and* its bundle id identifies a
    /// known Chromium browser do we send `AXEnhancedUserInterface`, the stronger
    /// screen-reader-mode signal Chrome's `BrowserCrApplication` listens for.
    /// On Sonoma+ Chrome waits two seconds after the last enhanced request
    /// before switching to complete (web) accessibility mode. Unknown/native
    /// apps never receive the enhanced signal.
    ///
    /// The mutex covers the attempt as well as the bookkeeping. Combined with
    /// the `STOP_REQUESTED` checks, this prevents a probe already in flight
    /// from setting an application back to true just after watcher teardown.
    /// Returns true only for the first successful activation of this pid —
    /// including a success that lands on a bounded retry after earlier
    /// unsupported attempts (see `should_attempt_activation`) — so the
    /// caller schedules exactly one delayed re-probe sequence and no thread
    /// storm. A pid that has already succeeded is never retried.
    unsafe fn maybe_activate_frontmost_application(
        application: AXUIElementRef,
        pid: i32,
        bundle_id: Option<&str>,
    ) -> bool {
        if pid == 0 || STOP_REQUESTED.load(Ordering::SeqCst) {
            return false;
        }

        let mut activations = AX_APPLICATION_ACTIVATIONS.lock().unwrap();
        let previous = activations
            .iter()
            .find(|activation| activation.pid == pid)
            .map(|activation| (activation.kind, activation.attempts));
        let already_succeeded = previous
            .is_some_and(|(kind, _)| !matches!(kind, ApplicationActivationKind::Unsupported));
        let attempts_so_far = previous.map(|(_, attempts)| attempts).unwrap_or(0);
        if already_succeeded
            || !should_attempt_activation(attempts_so_far)
            || STOP_REQUESTED.load(Ordering::SeqCst)
        {
            return false;
        }

        let manual_attr = match cfstring(AX_MANUAL_ACCESSIBILITY) {
            Ok(attr) => attr,
            Err(_) => return false,
        };
        let manual_err = AXUIElementSetAttributeValue(application, manual_attr, kCFBooleanTrue);
        CFRelease(manual_attr as CFTypeRef);

        let (enhanced_err, kind) = if manual_err == AX_ERROR_SUCCESS {
            (None, ApplicationActivationKind::ManualAccessibility)
        } else if manual_err == AX_ERROR_ATTRIBUTE_UNSUPPORTED
            && bundle_id.is_some_and(is_chromium_browser_bundle_id)
        {
            let enhanced_attr = match cfstring(AX_ENHANCED_USER_INTERFACE) {
                Ok(attr) => attr,
                Err(_) => return false,
            };
            let enhanced_err =
                AXUIElementSetAttributeValue(application, enhanced_attr, kCFBooleanTrue);
            CFRelease(enhanced_attr as CFTypeRef);
            let kind = if enhanced_err == AX_ERROR_SUCCESS {
                ApplicationActivationKind::EnhancedUserInterface
            } else {
                ApplicationActivationKind::Unsupported
            };
            (Some(enhanced_err), kind)
        } else {
            (None, ApplicationActivationKind::Unsupported)
        };

        let attempts = attempts_so_far + 1;
        match activations.iter_mut().find(|activation| activation.pid == pid) {
            Some(existing) => {
                existing.kind = kind;
                existing.attempts = attempts;
            }
            None => activations.push(ApplicationActivation { pid, kind, attempts }),
        }
        let enhanced_result = enhanced_err
            .map(|err| format!("err={err}"))
            .unwrap_or_else(|| "not attempted".to_string());
        let bundle_id = bundle_id.unwrap_or("<unknown>");
        crate::trace(&format!(
            "selection_watcher::maybe_activate_frontmost_application(mac): main_pid={pid} bundle_id={bundle_id} AXManualAccessibility err={manual_err}, AXEnhancedUserInterface {enhanced_result}"
        ));
        !matches!(kind, ApplicationActivationKind::Unsupported)
    }

    /// Chrome does not enable its web accessibility tree immediately after
    /// accepting AXEnhancedUserInterface, and the tree can finish warming up
    /// at any point along a range of delays rather than one fixed instant.
    /// Sleep on a single short-lived helper thread, never on the event-tap or
    /// selection worker, walking `APPLICATION_ACTIVATION_REPROBE_SCHEDULE` in
    /// order and enqueuing one ordinary recheck after each sleep. Capturing
    /// `tx` once up front (the same `NOTIFY_TX` clone the old one-shot
    /// version captured) means a watcher stop()/restart() in the middle of
    /// this schedule harmlessly disconnects the channel: the next `send`
    /// fails and the loop exits immediately instead of sleeping through the
    /// rest of the schedule for no reason.
    fn schedule_activation_reprobe() {
        let Some(tx) = NOTIFY_TX.lock().unwrap().as_ref().cloned() else {
            return;
        };
        std::thread::spawn(move || {
            for delay in APPLICATION_ACTIVATION_REPROBE_SCHEDULE {
                std::thread::sleep(delay);
                if tx.send(WorkerMsg::Recheck).is_err() {
                    break;
                }
            }
        });
    }

    /// Balance successful application-level activations when the selection
    /// watcher is disabled or Accessibility permission is revoked. In
    /// particular, this avoids leaving Chrome in complete AX mode indefinitely
    /// after reWrite no longer has a passive watcher running.
    unsafe fn deactivate_frontmost_applications() {
        let activations = {
            let mut active = AX_APPLICATION_ACTIVATIONS.lock().unwrap();
            std::mem::take(&mut *active)
        };

        for activation in activations {
            let attribute = match activation.kind {
                ApplicationActivationKind::EnhancedUserInterface => AX_ENHANCED_USER_INTERFACE,
                ApplicationActivationKind::ManualAccessibility => AX_MANUAL_ACCESSIBILITY,
                ApplicationActivationKind::Unsupported => continue,
            };
            let application = AXUIElementCreateApplication(activation.pid);
            if application.is_null() {
                continue;
            }
            if let Ok(attr) = cfstring(attribute) {
                let err = AXUIElementSetAttributeValue(application, attr, kCFBooleanFalse);
                CFRelease(attr as CFTypeRef);
                crate::trace(&format!(
                    "selection_watcher::deactivate_frontmost_applications(mac): pid={} {attribute}=false err={err}",
                    activation.pid
                ));
            }
            CFRelease(application as CFTypeRef);
        }
    }

    /// `(pid, attempt count)` pairs for pids sent the `AXManualAccessibility`
    /// activation below. Never evicted: once a process has exhausted its
    /// attempts (see `should_attempt_activation`/`MAX_ACTIVATION_ATTEMPTS_PER_PID`),
    /// asking again is a harmless no-op, so there's no need to clean this up
    /// e.g. when a browser quits — a reused pid from a long-exited process
    /// just gets a few redundant, harmless attempts at most.
    static AX_ACTIVATED_PIDS: Mutex<Vec<(i32, u32)>> = Mutex::new(Vec::new());

    /// Chromium-based apps (Chrome, Edge, Brave, Arc, Vivaldi, and Electron
    /// apps like VS Code/Notion) only populate the full accessibility tree
    /// that `AXSelectedText`/`AXSelectedTextRange`/`AXBoundsForRange` read
    /// from once they detect an actively-watching assistive-technology
    /// client — normally VoiceOver. This module's passive, read-only AX
    /// polling never triggers that, so on a fresh browser process every
    /// selection read finds a real focused/hit-tested element but no
    /// selection attributes on it, forever. Confirmed by live testing
    /// 2026-07-12: Notes detected selections correctly on the first try;
    /// Chrome and Notion (Electron) returned no selection on every attempt
    /// across a multi-minute session, with `probe_selection`'s two lookups
    /// both finding *an* element, just never one with selection data.
    ///
    /// The fix is Chromium's own documented escape hatch: setting
    /// `AXManualAccessibility` to `true` forces full accessibility mode on
    /// for that process, the same as if VoiceOver had started watching it.
    /// **Where to set it matters and was wrong in the first attempt at this
    /// fix**: setting it on the app-level `AXUIElementCreateApplication(pid)`
    /// object returned `kAXErrorAttributeUnsupported` (-25205) from real
    /// Chrome in live testing — Chrome's generic top-level Application
    /// accessibility object doesn't implement this Chromium-specific
    /// attribute at all. It has to be set on an element that actually lives
    /// inside Chrome's own accessibility bridge — i.e. the exact
    /// focused/hit-tested `AXUIElementRef` this function already has in hand
    /// from `probe_selection`, before it's released. Native, non-Chromium
    /// apps don't recognize this attribute on their elements either, and
    /// `AXUIElementSetAttributeValue` just returns an error there too
    /// (discarded) — so this is safe to attempt unconditionally on any
    /// element this module ever failed to read a selection from; no
    /// bundle-id/browser allowlist is needed.
    ///
    /// One real limitation even once this targets the right element:
    /// activating the tree doesn't make it exist instantly. The very first
    /// selection in a given browser process right after this fires may still
    /// not produce a bubble; the tree populates in the background, and the
    /// *next* selection in that same process should work, since the
    /// activation (and the tree) persists for the process's lifetime — this
    /// function asks up to `MAX_ACTIVATION_ATTEMPTS_PER_PID` times per pid,
    /// once per selection attempt that comes up empty for it, so a first
    /// attempt made while the process's AX bridge was still cold gets a few
    /// more chances instead of a permanent lockout.
    unsafe fn maybe_activate_manual_accessibility(element: AXUIElementRef) {
        let mut pid: i32 = 0;
        if AXUIElementGetPid(element, &mut pid) != AX_ERROR_SUCCESS || pid == 0 {
            return;
        }
        {
            let mut activated = AX_ACTIVATED_PIDS.lock().unwrap();
            match activated.iter_mut().find(|(activated_pid, _)| *activated_pid == pid) {
                Some((_, attempts)) => {
                    if !should_attempt_activation(*attempts) {
                        return;
                    }
                    *attempts += 1;
                }
                None => activated.push((pid, 1)),
            }
        }

        let attr = match cfstring(AX_MANUAL_ACCESSIBILITY) {
            Ok(a) => a,
            Err(_) => return,
        };
        let err = AXUIElementSetAttributeValue(element, attr, kCFBooleanTrue);
        CFRelease(attr as CFTypeRef);
        crate::trace(&format!(
            "selection_watcher::maybe_activate_manual_accessibility(mac): pid={pid} set AXManualAccessibility on found element -> err={err}"
        ));
    }

    /// Pure decision behind the zero-rect fallback in `selection_from_element`:
    /// given the last recorded left-mouse-up point, build a zero-size `CGRect`
    /// anchored there, or `None` if there's no usable point yet (both
    /// coordinates still at the atomics' initial/unset value of 0.0 — see
    /// `LAST_MOUSE_UP_X`/`LAST_MOUSE_UP_Y`). `selection_anchor_from_rect`
    /// turns a zero-size rect at `(x, y)` into exactly `(x, y)`, so this
    /// anchors the bubble at the mouse-up point instead of at AX bounds.
    /// Split out (and kept free of the atomic reads themselves) so it's
    /// directly unit-testable — see `mod tests` below.
    fn mouse_up_anchor_rect(x: f64, y: f64) -> Option<CGRect> {
        if x == 0.0 && y == 0.0 {
            return None;
        }
        Some(CGRect {
            origin: CGPoint { x, y },
            size: CGSize::default(),
        })
    }

    unsafe fn selection_from_element(
        element: AXUIElementRef,
    ) -> Result<Option<(String, CGRect)>, String> {
        if !is_element_editable(element) {
            return Ok(None);
        }

        let mut selected_text: CFTypeRef = std::ptr::null();
        let attr = cfstring(AX_SELECTED_TEXT)?;
        let err = AXUIElementCopyAttributeValue(element, attr, &mut selected_text);
        CFRelease(attr as CFTypeRef);
        if err != AX_ERROR_SUCCESS || selected_text.is_null() {
            return Ok(None);
        }
        let text = cfstring_to_string(selected_text as CFStringRef)?;
        CFRelease(selected_text);
        if text.is_empty() {
            return Ok(None);
        }

        // During Chromium/Electron's asynchronous AX-tree warm-up (see
        // `maybe_activate_manual_accessibility`/`schedule_activation_reprobe`),
        // `AXSelectedText` can already read correctly while
        // `AXBoundsForRange` is still transiently empty (missing entirely, or
        // a real-but-zero-size rect). Previously that discarded a selection
        // whose *text* was read fine. Instead, fall back to anchoring at the
        // last mouse-up point — mirrors the Windows backend's tolerance for
        // an imprecise anchor over no bubble at all. Only take this path when
        // there's a usable mouse-up point; otherwise keep discarding, same as
        // before.
        let rect = match selected_range_bounds(element)? {
            Some(rect) if rect.size.width > 0.0 && rect.size.height > 0.0 => rect,
            _ => {
                let x = f64::from_bits(LAST_MOUSE_UP_X.load(Ordering::Relaxed));
                let y = f64::from_bits(LAST_MOUSE_UP_Y.load(Ordering::Relaxed));
                match mouse_up_anchor_rect(x, y) {
                    Some(rect) => rect,
                    None => return Ok(None),
                }
            }
        };
        Ok(Some((text, rect)))
    }

    unsafe fn selected_range_bounds(element: AXUIElementRef) -> Result<Option<CGRect>, String> {
        let mut range_value: CFTypeRef = std::ptr::null();
        let attr = cfstring(AX_SELECTED_TEXT_RANGE)?;
        let err = AXUIElementCopyAttributeValue(element, attr, &mut range_value);
        CFRelease(attr as CFTypeRef);
        if err != AX_ERROR_SUCCESS || range_value.is_null() {
            return Ok(None);
        }

        let retained_range = CFRetain(range_value);
        let mut bounds_value: CFTypeRef = std::ptr::null();
        let bounds_attr = cfstring(AX_BOUNDS_FOR_RANGE)?;
        let bounds_err = AXUIElementCopyParameterizedAttributeValue(
            element,
            bounds_attr,
            retained_range,
            &mut bounds_value,
        );
        CFRelease(bounds_attr as CFTypeRef);
        CFRelease(retained_range);
        CFRelease(range_value);
        if bounds_err != AX_ERROR_SUCCESS || bounds_value.is_null() {
            return Ok(None);
        }

        let mut rect = CGRect::default();
        let ok = AXValueGetValue(
            bounds_value as AXValueRef,
            AX_VALUE_CGRECT,
            &mut rect as *mut CGRect as *mut c_void,
        );
        CFRelease(bounds_value);
        if ok == 0 {
            return Ok(None);
        }
        Ok(Some(rect))
    }

    unsafe fn cfstring(value: &str) -> Result<CFStringRef, String> {
        let c_value = CString::new(value).map_err(|e| e.to_string())?;
        let cf =
            CFStringCreateWithCString(std::ptr::null(), c_value.as_ptr(), CF_STRING_ENCODING_UTF8);
        if cf.is_null() {
            return Err(format!("CFStringCreateWithCString failed for {value}"));
        }
        Ok(cf)
    }

    unsafe fn cfstring_to_string(value: CFStringRef) -> Result<String, String> {
        let len = CFStringGetLength(value);
        if len <= 0 {
            return Ok(String::new());
        }
        let max_len = CFStringGetMaximumSizeForEncoding(len, CF_STRING_ENCODING_UTF8) + 1;
        let mut buffer = vec![0_u8; max_len as usize];
        let ok = CFStringGetCString(
            value,
            buffer.as_mut_ptr() as *mut c_char,
            max_len,
            CF_STRING_ENCODING_UTF8,
        );
        if ok == 0 {
            return Err("CFStringGetCString failed".to_string());
        }
        let nul = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
        String::from_utf8(buffer[..nul].to_vec()).map_err(|e| e.to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn selection_significant_requires_min_chars_after_trim() {
            assert!(!is_selection_significant(""));
            assert!(!is_selection_significant("  "));
            assert!(!is_selection_significant("a"));
            assert!(!is_selection_significant(" a \n"));
            assert!(is_selection_significant("ab"));
            assert!(is_selection_significant("  ab  "));
            assert!(is_selection_significant("hello world"));
        }

        #[test]
        fn selection_significant_counts_chars_not_bytes() {
            // Multi-byte UTF-8 chars should count as one char each, not bytes.
            assert!(is_selection_significant("héllo"));
            assert!(!is_selection_significant("é")); // 1 char, 2 bytes
        }

        #[test]
        fn anchor_is_bottom_right_of_rect() {
            let rect = CGRect {
                origin: CGPoint { x: 100.0, y: 50.0 },
                size: CGSize {
                    width: 40.0,
                    height: 15.0,
                },
            };
            assert_eq!(selection_anchor_from_rect(rect), (140.0, 65.0));
        }

        #[test]
        fn anchor_handles_zero_size_rect() {
            let rect = CGRect {
                origin: CGPoint { x: 10.0, y: 20.0 },
                size: CGSize::default(),
            };
            assert_eq!(selection_anchor_from_rect(rect), (10.0, 20.0));
        }

        #[test]
        fn frontmost_switch_clears_when_pid_differs_from_own_and_source() {
            assert!(should_clear_for_frontmost_switch(100, 300, Some(200)));
        }

        #[test]
        fn frontmost_switch_does_not_clear_for_own_pid() {
            assert!(!should_clear_for_frontmost_switch(100, 300, Some(100)));
        }

        #[test]
        fn frontmost_switch_does_not_clear_for_source_pid() {
            // The steady state for the whole time the bubble is visible:
            // show_bubble/show_bubble_menu deliberately don't steal
            // foreground, so the source app (Safari, say) stays frontmost.
            // This must NOT clear the bubble.
            assert!(!should_clear_for_frontmost_switch(100, 300, Some(300)));
        }

        #[test]
        fn frontmost_switch_does_not_clear_when_unknown() {
            assert!(!should_clear_for_frontmost_switch(100, 300, None));
        }

        #[test]
        fn editable_role_accepts_known_editable_browser_roles() {
            assert!(is_editable_role("AXTextField"));
            assert!(is_editable_role("AXTextArea"));
            assert!(is_editable_role("AXComboBox"));
            assert!(is_editable_role("AXSearchField"));
        }

        #[test]
        fn editable_role_rejects_read_only_and_empty_roles() {
            assert!(!is_editable_role(""));
            assert!(!is_editable_role("AXStaticText"));
            assert!(!is_editable_role("AXWebArea"));
            assert!(!is_editable_role("AXGroup"));
        }

        #[test]
        fn chromium_browser_bundle_id_accepts_supported_browser_families() {
            for bundle_id in [
                "com.google.Chrome",
                "com.google.Chrome.beta",
                "com.google.Chrome.dev",
                "com.google.Chrome.canary",
                "org.chromium.Chromium",
                "com.microsoft.edgemac",
                "com.microsoft.edgemac.Beta",
                "com.brave.Browser",
                "com.brave.Browser.nightly",
                "company.thebrowser.Browser",
                "com.vivaldi.Vivaldi",
                "com.operasoftware.Opera",
                "com.operasoftware.OperaGX",
            ] {
                assert!(
                    is_chromium_browser_bundle_id(bundle_id),
                    "expected {bundle_id} to be recognized"
                );
            }
        }

        #[test]
        fn chromium_browser_bundle_id_rejects_native_unknown_and_electron_apps() {
            for bundle_id in [
                "",
                "com.apple.Notes",
                "com.apple.finder",
                "com.apple.Safari",
                "org.mozilla.firefox",
                "notion.id",
                "com.microsoft.VSCode",
                "com.tinyspeck.slackmacgap",
                "com.google.ChromeRemoteDesktopHost",
            ] {
                assert!(
                    !is_chromium_browser_bundle_id(bundle_id),
                    "expected {bundle_id} to be rejected"
                );
            }
        }

        #[test]
        fn mouse_up_anchor_rect_none_when_point_unset() {
            assert!(mouse_up_anchor_rect(0.0, 0.0).is_none());
        }

        #[test]
        fn mouse_up_anchor_rect_zero_size_at_point_when_usable() {
            let rect = mouse_up_anchor_rect(120.0, 340.0).expect("usable point");
            assert_eq!(rect.origin.x, 120.0);
            assert_eq!(rect.origin.y, 340.0);
            assert_eq!(rect.size.width, 0.0);
            assert_eq!(rect.size.height, 0.0);
            // A zero-size rect at (x, y) must resolve to exactly (x, y),
            // matching selection_from_element's intent of anchoring the
            // bubble at the mouse-up point.
            assert_eq!(selection_anchor_from_rect(rect), (120.0, 340.0));
        }

        #[test]
        fn mouse_up_anchor_rect_usable_when_only_one_axis_nonzero() {
            // Only "both still 0.0" (the atomics' unset state) should count
            // as unusable — a point sitting exactly on one screen edge (x==0
            // or y==0) is still a real point.
            assert!(mouse_up_anchor_rect(0.0, 50.0).is_some());
            assert!(mouse_up_anchor_rect(50.0, 0.0).is_some());
        }

        #[test]
        fn activation_attempts_allowed_below_cap() {
            assert!(should_attempt_activation(0));
            assert!(should_attempt_activation(1));
            assert!(should_attempt_activation(MAX_ACTIVATION_ATTEMPTS_PER_PID - 1));
        }

        #[test]
        fn activation_attempts_blocked_at_and_above_cap() {
            assert!(!should_attempt_activation(MAX_ACTIVATION_ATTEMPTS_PER_PID));
            assert!(!should_attempt_activation(MAX_ACTIVATION_ATTEMPTS_PER_PID + 5));
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::{focus_last_source_window, last_anchor, start, stop, AnchorPayload};
