//! System-level Escape-key dismissal for the rewrite overlay.
//!
//! `Overlay.tsx` also listens for Escape itself via a plain
//! `window.addEventListener("keydown", ...)`, but that only fires if the
//! overlay webview genuinely has OS keyboard focus — which isn't guaranteed
//! (see `show_overlay`'s `w.set_focus()` call in `lib.rs`, which can fail
//! silently or lose focus moments later). This module exists to be the
//! reliable, focus-independent fallback: while the overlay is visible, it
//! watches for Escape system-wide (Windows: a low-level keyboard hook;
//! macOS: a `CGEventTap`) and routes it through the overlay's own close
//! handler — the same one the X button uses — via the `overlay:esc` event,
//! rather than trying to hide the window directly from a thread that
//! doesn't own it.
//!
//! Both platform backends expose the same `start(app)` / `stop()` API and
//! the same idempotency contract: `start` is safe to call on every overlay
//! show (it must not double-install), and `stop` is safe to call even if
//! nothing is running.

#[cfg(target_os = "windows")]
mod win {
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
            crate::trace("esc_hook::start: already running");
            return; // already running
        }
        crate::trace("esc_hook::start: spawning hook thread");
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
                crate::trace("esc_hook::hook_proc: ESC keydown");
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
}

#[cfg(target_os = "windows")]
pub use win::{start, stop};

// ── macOS ────────────────────────────────────────────────────────────────
//
// Windows' `WH_KEYBOARD_LL` has no direct macOS analogue as an OS API, but a
// `CGEventTap` is the closest structural equivalent: a system-wide callback
// that observes (and can consume) keyboard events regardless of which app or
// window currently has focus, exactly the property this module needs — the
// whole point of it is to dismiss the overlay even when `show_overlay`'s
// `w.set_focus()` didn't actually land.
//
// This was chosen over `NSEvent addLocalMonitorForEventsMatchingMask:` /
// `addGlobalMonitorForEventsMatchingMask:` because those only solve a
// narrower problem: a local monitor only sees events already routed to one
// of *this app's* windows (no help if the overlay itself isn't key window),
// and a global monitor only *reports* events sent to other apps — it cannot
// consume them, so it couldn't swallow Escape the way the Windows hook does.
// A `CGEventTap` (in `kCGEventTapOptionDefault`, not `ListenOnly`) can do
// both: see Escape no matter who's focused, and eat it before it reaches
// whatever window is actually frontmost.
//
// Requirements/trade-offs of this choice:
//   - Needs Accessibility permission. This app already requires that for
//     `clipboard::capture_selection`, and the Phase 2 onboarding tutorial
//     (`AccessibilityView.tsx`) is expected to have it granted by the time
//     the overlay is ever shown — but `start()` re-checks
//     `clipboard::accessibility_trusted(false)` (non-prompting) every call
//     and simply skips installing the tap if it's not granted, rather than
//     erroring. In that degraded state the frontend's own `keydown` listener
//     in `Overlay.tsx` is the only Escape path, same as it always was before
//     this module existed. Because `start()` is called on every overlay
//     show, permission granted later in the same session is picked up
//     automatically on the next show — no extra polling needed here.
//   - Must run its callback off a `CFRunLoop` sourced on a dedicated thread
//     (mirroring the Windows module's `std::thread::spawn(run_hook_thread)` +
//     `GetMessageW` pump), not the Tauri/AppKit main thread — but the
//     callback itself must NEVER call into Tauri or AppKit directly, per the
//     lesson from this app's recent macOS main-thread crash fixes
//     (`foreground.rs`, `lib.rs`'s `remember_paste_target_window` /
//     `focus_paste_target_window`). The callback here does the absolute
//     minimum inline (check the keycode, decide whether to swallow the
//     event) and marshals everything else — checking overlay visibility,
//     tearing the tap down, emitting `overlay:esc` — onto the main thread via
//     `app.run_on_main_thread`, fire-and-forget, exactly like those fixes do.
//   - A slow tap callback risks macOS silently disabling the tap
//     (`kCGEventTapDisabledByTimeout`); the callback is kept trivial for
//     exactly this reason. If it still happens, the tap re-enables itself the
//     next time it receives that notification.
//
// No new Cargo.toml dependency: `CoreGraphics` and `CoreFoundation` are
// linked directly by symbol, the same pattern `clipboard.rs` already uses
// for `AXIsProcessTrusted`/`AXIsProcessTrustedWithOptions` (and
// `CoreGraphics` is already pulled into the link step transitively via
// `enigo`'s macOS backend, which also drives `CGEvent`).
#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
    use std::sync::{Mutex, OnceLock};
    use tauri::{AppHandle, Emitter, Manager};

    // ── Raw CoreFoundation / CoreGraphics FFI ───────────────────────────────
    type CFAllocatorRef = *const c_void;
    type CFIndex = isize;
    type CFTimeInterval = f64;
    type CFRunLoopRef = *mut c_void;
    type CFRunLoopSourceRef = *mut c_void;
    type CFStringRef = *const c_void;
    type CFMachPortRef = *mut c_void;
    type CFTypeRef = *const c_void;
    type CGEventRef = *mut c_void;
    type CGEventTapProxy = *const c_void;
    type CGEventMask = u64;

    type CGEventTapCallBack = unsafe extern "C" fn(
        proxy: CGEventTapProxy,
        etype: u32,
        event: CGEventRef,
        user_info: *mut c_void,
    ) -> CGEventRef;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFRunLoopCommonModes: CFStringRef;
        static kCFRunLoopDefaultMode: CFStringRef;
        fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        // Bounded poll instead of a single blocking `CFRunLoopRun()` — see the
        // `run_tap_thread` loop below for why (a `CFRunLoopStop` call that
        // lands before the loop has an active run frame is silently a no-op,
        // so a plain `CFRunLoopRun()` can hang forever with the tap still
        // live and swallowing Escape system-wide).
        fn CFRunLoopRunInMode(
            mode: CFStringRef,
            seconds: CFTimeInterval,
            return_after_source_handled: bool,
        ) -> i32;
        fn CFRunLoopStop(rl: CFRunLoopRef);
        fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
        fn CFRunLoopRemoveSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
        fn CFMachPortCreateRunLoopSource(
            allocator: CFAllocatorRef,
            port: CFMachPortRef,
            order: CFIndex,
        ) -> CFRunLoopSourceRef;
        fn CFMachPortInvalidate(port: CFMachPortRef);
        fn CFRelease(cf: CFTypeRef);
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
        fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    }

    // Quartz Event Services constants (CGEventTypes.h / CGEventTapCreate).
    const KCG_SESSION_EVENT_TAP: u32 = 1; // kCGSessionEventTap
    const KCG_HEAD_INSERT_EVENT_TAP: u32 = 0; // kCGHeadInsertEventTap
    const KCG_EVENT_TAP_OPTION_DEFAULT: u32 = 0; // kCGEventTapOptionDefault (active filter, can consume)
    const KCG_EVENT_KEY_DOWN: u32 = 10; // kCGEventKeyDown
    const KCG_EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFFFFFE; // kCGEventTapDisabledByTimeout
    const KCG_EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFFFFFF; // kCGEventTapDisabledByUserInput
    const KCG_KEYBOARD_EVENT_KEYCODE: u32 = 9; // kCGKeyboardEventKeycode (CGEventField)
    const VK_ESCAPE: i64 = 0x35; // Escape virtual keycode

    /// How often the poll loop in `run_tap_thread` wakes up to re-check
    /// whether `stop()` has been requested. Bounds worst-case shutdown
    /// latency (see that loop's comment for why this exists at all instead
    /// of a single blocking `CFRunLoopRun()`).
    const POLL_INTERVAL_SECS: f64 = 0.25;

    static APP: OnceLock<AppHandle> = OnceLock::new();

    /// Set by `stop()`, cleared by `run_tap_thread` once it has a live tap.
    /// The poll loop below re-checks this every `POLL_INTERVAL_SECS` — this
    /// is the *reliable* shutdown signal; `CFRunLoopStop` (also called by
    /// `stop()`) is just a best-effort fast path on top of it, since it's a
    /// no-op if it lands before the run loop has an active frame.
    static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

    /// Holds the real `CFMachPortRef` returned by `CGEventTapCreate`, once
    /// known, so `tap_callback` can re-enable the *actual* tap after a
    /// `kCGEventTapDisabledByTimeout`/`kCGEventTapDisabledByUserInput`
    /// notification. A pointer to this static is threaded through
    /// `CGEventTapCreate`'s `user_info` parameter (see `run_tap_thread`) —
    /// the tap's own `proxy` callback parameter is a distinct, opaque token
    /// documented only for `CGEventTapPostEvent` and is NOT valid to pass to
    /// `CGEventTapEnable`, despite having a compatible pointer type that a
    /// cast will happily paper over. One static suffices because at most one
    /// tap is ever live at a time (serialized by `TAP` below).
    static TAP_REF_SLOT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

    /// Lifecycle state for the event tap, serialized behind one mutex so
    /// start()/stop() can't race each other the way a bare pair of atomics did
    /// on Windows (see that module's own comment about the hook-thread race
    /// this design avoids repeating).
    enum TapState {
        /// Nothing installed, no thread in flight.
        Idle,
        /// Thread spawned, but it hasn't published its `CFRunLoopRef` yet.
        Starting,
        /// Tap installed and its run loop is live; holds the `CFRunLoopRef`
        /// (as a `usize` — `CFRunLoopRef` itself isn't `Send`, but the C API
        /// documents `CFRunLoopStop` as safe to call from any thread on a
        /// run loop object obtained from another thread, which is exactly
        /// how `stop()` uses it).
        Running(usize),
    }

    static TAP: Mutex<TapState> = Mutex::new(TapState::Idle);

    /// Install the CGEventTap if it isn't already running (or already
    /// attempting to start). Safe to call on every overlay show — see the
    /// module doc comment above for the full design rationale.
    ///
    /// Degrades gracefully when Accessibility permission isn't granted: skips
    /// installing the tap entirely rather than erroring, leaving
    /// `Overlay.tsx`'s own frontend `keydown` listener as the only Escape
    /// path (the same as before this module existed). Because this runs on
    /// every overlay show, permission granted later in the session is picked
    /// up on the very next show with no extra polling required here.
    pub fn start(app: &AppHandle) {
        let _ = APP.get_or_init(|| app.clone());

        {
            let mut state = TAP.lock().unwrap();
            if !matches!(*state, TapState::Idle) {
                crate::trace("esc_hook::start: already running");
                return;
            }
            if !crate::clipboard::accessibility_trusted(false) {
                crate::trace(
                    "esc_hook::start: Accessibility not granted, skipping CGEventTap install \
                     (frontend Escape listener in Overlay.tsx remains the only fallback)",
                );
                return; // stays Idle; next show_overlay call will re-check
            }
            *state = TapState::Starting;
        }

        crate::trace("esc_hook::start: spawning CGEventTap thread");
        std::thread::spawn(run_tap_thread);
    }

    /// Uninstall the tap by signalling its dedicated poll loop to stop. Safe
    /// to call even if nothing is running or the tap is still mid-startup.
    ///
    /// Sets `STOP_REQUESTED` and calls `CFRunLoopStop` in the same critical
    /// section as the `TapState` transition (both guarded by `TAP`'s lock),
    /// so this can never race with `run_tap_thread` publishing `Running` and
    /// clearing `STOP_REQUESTED` — whichever happens first fully completes
    /// before the other starts. `CFRunLoopStop` is a best-effort fast path
    /// (it only actually takes effect if the run loop currently has an
    /// active frame); `STOP_REQUESTED` is what guarantees termination within
    /// one `POLL_INTERVAL_SECS` even when `CFRunLoopStop`'s call lands in the
    /// gap before the loop has started spinning (a real race on a fast
    /// overlay open-then-close cycle — see `run_tap_thread`'s poll loop).
    pub fn stop() {
        let mut state = TAP.lock().unwrap();
        match *state {
            TapState::Idle => {}
            TapState::Starting => {
                // The thread hasn't published its run loop yet. Flip back to
                // Idle now; `run_tap_thread` checks for exactly this before it
                // ever enters the poll loop, and tears itself down instead.
                *state = TapState::Idle;
            }
            TapState::Running(rl_ptr) => {
                STOP_REQUESTED.store(true, Ordering::SeqCst);
                unsafe { CFRunLoopStop(rl_ptr as CFRunLoopRef) };
                *state = TapState::Idle;
            }
        }
    }

    fn run_tap_thread() {
        unsafe {
            let mask: CGEventMask = 1u64 << (KCG_EVENT_KEY_DOWN as u64);
            // Pass a pointer to TAP_REF_SLOT (a stable 'static address) as
            // user_info. We don't know the real tap handle yet — that's the
            // return value of this very call — so the callback can't be
            // handed the handle directly; instead it's handed a slot it can
            // read *after* we fill it in below. This is what lets
            // tap_callback re-enable the real tap on a disabled-by-timeout
            // notification instead of misusing the unrelated `proxy` param.
            let user_info = &TAP_REF_SLOT as *const AtomicPtr<c_void> as *mut c_void;
            let tap_ref = CGEventTapCreate(
                KCG_SESSION_EVENT_TAP,
                KCG_HEAD_INSERT_EVENT_TAP,
                KCG_EVENT_TAP_OPTION_DEFAULT,
                mask,
                tap_callback,
                user_info,
            );

            if tap_ref.is_null() {
                // Most likely Accessibility permission was revoked between
                // start()'s check and here, or the tap limit was hit. Reset to
                // Idle so a later start() (e.g. after permission is re-granted)
                // can try again.
                crate::trace("esc_hook::run_tap_thread: CGEventTapCreate failed, leaving Idle");
                *TAP.lock().unwrap() = TapState::Idle;
                return;
            }

            // Publish the real handle before anything can start relying on
            // it (the tap isn't enabled yet, so the callback can't fire
            // before this point).
            TAP_REF_SLOT.store(tap_ref, Ordering::SeqCst);

            let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap_ref, 0);
            let rl = CFRunLoopGetCurrent();

            // Publish the run loop handle so stop() can signal us — but only if
            // nobody called stop() while we were still starting up.
            {
                let mut state = TAP.lock().unwrap();
                if !matches!(*state, TapState::Starting) {
                    crate::trace(
                        "esc_hook::run_tap_thread: stop() ran during startup, tearing down without entering the run loop",
                    );
                    drop(state);
                    TAP_REF_SLOT.store(std::ptr::null_mut(), Ordering::SeqCst);
                    CFMachPortInvalidate(tap_ref);
                    CFRelease(source as CFTypeRef);
                    CFRelease(tap_ref as CFTypeRef);
                    return;
                }
                *state = TapState::Running(rl as usize);
                // Same critical section as the state write above, so this can
                // never race a concurrent stop() setting it true (see stop()'s
                // doc comment) — whichever runs first, runs to completion
                // before the other can observe the lock.
                STOP_REQUESTED.store(false, Ordering::SeqCst);
            }

            CFRunLoopAddSource(rl, source, kCFRunLoopCommonModes);
            CGEventTapEnable(tap_ref, true);
            crate::trace("esc_hook::run_tap_thread: CGEventTap installed, entering poll loop");

            // Bounded poll instead of a single blocking `CFRunLoopRun()`.
            // `CFRunLoopStop` only takes effect while the run loop has an
            // active frame; calling it in the gap between "state says
            // Running" and "the loop has actually started spinning" (a real
            // window on a fast overlay open-then-close cycle) is silently a
            // no-op, which would otherwise hang this thread in
            // `CFRunLoopRun()` forever with the tap still live and
            // swallowing Escape system-wide. Re-checking STOP_REQUESTED every
            // POLL_INTERVAL_SECS guarantees we notice a stop within one
            // interval regardless of whether CFRunLoopStop's signal landed.
            loop {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, POLL_INTERVAL_SECS, false);
                if STOP_REQUESTED.load(Ordering::SeqCst) {
                    break;
                }
                // Defensive secondary check: if TAP no longer reflects this
                // generation's run loop at all (shouldn't happen given
                // STOP_REQUESTED is set in the same critical section as the
                // state write, but cheap insurance against future edits).
                let state = TAP.lock().unwrap();
                if !matches!(*state, TapState::Running(ptr) if ptr == rl as usize) {
                    break;
                }
            }

            crate::trace("esc_hook::run_tap_thread: poll loop stopped, tearing down");
            CGEventTapEnable(tap_ref, false);
            // Explicit removal before invalidating/releasing, rather than
            // relying on CFMachPortInvalidate to implicitly detach the
            // derived run-loop source — keeps teardown order independent of
            // that (correct but implicit) behavior if this code is ever
            // reordered later.
            CFRunLoopRemoveSource(rl, source, kCFRunLoopCommonModes);
            CFMachPortInvalidate(tap_ref);
            CFRelease(source as CFTypeRef);
            CFRelease(tap_ref as CFTypeRef);
            TAP_REF_SLOT.store(std::ptr::null_mut(), Ordering::SeqCst);

            // Only clear state if it's still ours to clear — a belt-and-
            // suspenders check against the exact kind of stale-generation race
            // that bit the Windows hook thread (see that module's start() doc
            // comment). In practice stop() already set Idle before signalling
            // us, and start() never spawns a new thread until state is Idle, so
            // this should always match; if it somehow doesn't, a newer
            // generation owns the slot and we must not touch it.
            let mut state = TAP.lock().unwrap();
            if matches!(*state, TapState::Running(ptr) if ptr == rl as usize) {
                *state = TapState::Idle;
            }
        }
    }

    /// The tap callback. Kept intentionally trivial per Apple's guidance (a
    /// slow callback risks the OS silently disabling the tap): it only reads
    /// the keycode and decides whether to consume the event. Everything that
    /// touches Tauri or AppKit — checking overlay visibility, tearing down the
    /// tap, emitting `overlay:esc` — is marshaled onto the main thread via
    /// `run_on_main_thread`, fire-and-forget, exactly like the main-thread
    /// fixes in `foreground.rs` / `lib.rs`. This callback runs on the
    /// dedicated CFRunLoop thread spawned by `run_tap_thread`, never the main
    /// thread, so it must not call into Tauri/AppKit directly.
    unsafe extern "C" fn tap_callback(
        _proxy: CGEventTapProxy,
        etype: u32,
        event: CGEventRef,
        user_info: *mut c_void,
    ) -> CGEventRef {
        if etype == KCG_EVENT_TAP_DISABLED_BY_TIMEOUT
            || etype == KCG_EVENT_TAP_DISABLED_BY_USER_INPUT
        {
            // The OS disabled the tap (commonly because a callback took too
            // long — the very first run_on_main_thread marshal below is a
            // realistic trigger if the main loop/webview is busy right at
            // overlay-show time). Re-enable using the REAL tap handle read
            // back out of user_info — NOT the `proxy` parameter. `proxy` is a
            // distinct, opaque `CGEventTapProxy` token Apple documents as
            // valid only for `CGEventTapPostEvent`; it is not the
            // `CFMachPortRef` `CGEventTapEnable` requires, even though the
            // pointer types happen to cast against each other without
            // complaint. Passing it there would silently fail to re-enable,
            // permanently killing native Escape dismissal after a single
            // transient disable (start() would never notice, since as far as
            // its TapState is concerned the tap is still `Running`).
            crate::trace("esc_hook::tap_callback: tap disabled by OS, re-enabling");
            if !user_info.is_null() {
                let slot = &*(user_info as *const AtomicPtr<c_void>);
                let real_tap = slot.load(Ordering::SeqCst);
                if !real_tap.is_null() {
                    CGEventTapEnable(real_tap as CFMachPortRef, true);
                }
            }
            return event;
        }

        if etype == KCG_EVENT_KEY_DOWN {
            let keycode = CGEventGetIntegerValueField(event, KCG_KEYBOARD_EVENT_KEYCODE);
            if keycode == VK_ESCAPE {
                crate::trace("esc_hook::tap_callback: ESC keydown");
                if let Some(app) = APP.get() {
                    let handle = app.clone();
                    // Fire-and-forget: the swallow decision below doesn't wait
                    // on this. Mirrors the Windows hook trusting that "the tap
                    // is installed" already means "the overlay is meant to be
                    // visible right now" (start()/stop() are scoped tightly to
                    // show_overlay / the overlay's CloseRequested handler).
                    let _ = app.run_on_main_thread(move || {
                        if let Some(w) = handle.get_webview_window("overlay") {
                            if w.is_visible().unwrap_or(false) {
                                stop();
                                let _ = handle.emit_to("overlay", "overlay:esc", ());
                            }
                        }
                    });
                }
                // Consume the keypress (return NULL) so it doesn't also reach
                // whatever window is actually frontmost — the same swallow
                // behavior the Windows hook's `return 1` produces.
                return std::ptr::null_mut();
            }
        }

        event
    }
}

#[cfg(target_os = "macos")]
pub use mac::{start, stop};
