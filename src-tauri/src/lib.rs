#[cfg(target_os = "macos")]
use std::sync::atomic::AtomicI32;
#[cfg(target_os = "windows")]
use std::sync::atomic::AtomicIsize;
use std::{
    hash::{Hash, Hasher},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
};
#[cfg(target_os = "macos")]
use tauri::LogicalPosition;
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Listener, LogicalSize, Manager, PhysicalPosition, WebviewWindow,
};

pub mod auth;
pub mod clipboard;
pub mod commands;
pub mod config;
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub mod esc_hook;
pub mod foreground;
pub mod history;
pub mod rewrite;
pub mod secure_store;
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub mod selection_watcher;
pub mod skills;
pub mod sync;

/// Visible size (logical px) of the bubble ring drawn in Bubble.tsx. Kept in
/// sync with that component's own hardcoded size.
const BUBBLE_VISIBLE_SIZE: f64 = 30.0;

/// Offset from the UIA selection anchor to the visible bubble's center. A small
/// positive X keeps it just past the selection edge; a negative Y tucks it
/// slightly above the bottom of the highlight so it reads as attached.
const BUBBLE_ANCHOR_CENTER_OFFSET_X: f64 = 6.0;
const BUBBLE_ANCHOR_CENTER_OFFSET_Y: f64 = -8.0;

/// Extra invisible margin (logical px) added on each side of the bubble
/// window beyond the visible ring (see Bubble.tsx), to make the actual click
/// target more forgiving than the visible dot alone. See `show_bubble` and
/// the bubble window pre-warm block for how this is applied.
const BUBBLE_HIT_PADDING: f64 = 10.0;

const BUBBLE_MENU_WIDTH: f64 = 168.0;
const BUBBLE_MENU_HEIGHT: f64 = 180.0;
const BUBBLE_MENU_CLOSE_SUPPRESS_MS: u64 = 500;
const BUBBLE_MENU_OPEN_CLICK_GRACE_MS: u64 = 350;
pub const BUBBLE_MENU_PARKED_X: f64 = -32000.0;
pub const BUBBLE_MENU_PARKED_Y: f64 = -32000.0;
static BUBBLE_MENU_SUPPRESS_PROBE_UNTIL_MS: AtomicU64 = AtomicU64::new(0);
static BUBBLE_MENU_IGNORE_OUTSIDE_CLICK_UNTIL_MS: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "macos")]
static PASTE_TARGET_PID: AtomicI32 = AtomicI32::new(0);
#[cfg(target_os = "windows")]
static PASTE_TARGET_HWND: AtomicIsize = AtomicIsize::new(0);
static PASTE_TRACE_ID: AtomicU64 = AtomicU64::new(1);

pub fn is_bubble_menu_parked(x: i32, y: i32) -> bool {
    (x - BUBBLE_MENU_PARKED_X as i32).abs() < 100 && (y - BUBBLE_MENU_PARKED_Y as i32).abs() < 100
}

// ── Temporary diagnostics (overlay first-open hang) ─────────────────────────
// Timestamped, thread-tagged tracing to pinpoint where the main event loop
// stalls on the first couple of overlay opens. Remove once the hang is fixed.
use std::sync::OnceLock;
static TRACE_START: OnceLock<std::time::Instant> = OnceLock::new();

/// The OS thread `run()` started on, captured once at startup. `run()` is
/// invoked from `main()` before Tauri's event loop starts, and the event
/// loop then runs on that same calling thread — so this is a reliable stand-in
/// for "the main thread" without needing platform-specific APIs.
///
/// This exists to answer an open question about the macOS crash fix in
/// `foreground::detect`/`remember_paste_target_window`/
/// `focus_paste_target_window`: those used to call AppKit's `NSWorkspace`/
/// `NSRunningApplication` directly from `on_hotkey`/`on_super_hotkey`, which
/// is not *guaranteed* to run on the main thread, and that was blamed for a
/// crash. They now marshal through `run_on_main_thread`. But it's possible
/// the global-shortcut handler was already running on the main thread all
/// along, in which case that marshaling is a no-op and the real crash cause
/// is still unknown. `is_main_thread()` plus the `tid=` field `trace()`
/// already logs on every line let a human confirm from real trace output
/// which thread actually made the AppKit calls. See `project.md` Known Gaps.
static MAIN_THREAD_ID: OnceLock<std::thread::ThreadId> = OnceLock::new();

/// Whether the current thread is the one `run()` started on. See
/// `MAIN_THREAD_ID` for why this exists.
pub fn is_main_thread() -> bool {
    MAIN_THREAD_ID.get() == Some(&std::thread::current().id())
}

/// Latched true the moment the user genuinely opens the overlay. The startup
/// webview-warming pass shows the overlay off-screen and then hides it again —
/// once via an `overlay:ready` listener, once via a 5s fallback timer. On a cold
/// webview `overlay:ready` can arrive *seconds after* the first real open (12.6s
/// in one trace), so those hides would fire while the user is looking at the
/// overlay and yank it away — which is exactly the "overlay crashes on the first
/// opens after launch" symptom. Both warm-pass hides check this first and skip
/// once a real open has happened. See `show_overlay` and the warm block.
static OVERLAY_OPENED: AtomicBool = AtomicBool::new(false);
pub fn trace(where_: &str) {
    let t0 = TRACE_START.get_or_init(std::time::Instant::now);
    eprintln!(
        "[trace +{:>8.3}s tid={:?}] {}",
        t0.elapsed().as_secs_f64(),
        std::thread::current().id(),
        where_
    );
}

pub fn next_paste_trace_id() -> u64 {
    PASTE_TRACE_ID.fetch_add(1, Ordering::SeqCst)
}

pub fn text_fingerprint(text: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    format!("len={} hash={:016x}", text.len(), hasher.finish())
}

#[cfg(target_os = "windows")]
pub fn remember_paste_target_window(_app: &AppHandle) {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let hwnd = unsafe { GetForegroundWindow().0 as isize };
    PASTE_TARGET_HWND.store(hwnd, Ordering::SeqCst);
    trace(&format!("remember_paste_target_window: hwnd={hwnd}"));
}

/// Like `foreground::detect`, this can be called from `on_hotkey`/
/// `on_super_hotkey` off the main app thread — calling AppKit's
/// `NSWorkspace` directly there is the same class of bug that used to crash
/// `foreground::detect`. Marshal onto the main thread via
/// `run_on_main_thread`. Unlike `detect`, nothing needs the result back
/// synchronously (`PASTE_TARGET_PID` is only read later, from
/// `focus_paste_target_window`, well after the async capture/rewrite work
/// has had time to run), so this is fire-and-forget rather than blocking on
/// a channel.
#[cfg(target_os = "macos")]
pub fn remember_paste_target_window(app: &AppHandle) {
    use objc2_app_kit::NSWorkspace;

    let _ = app.run_on_main_thread(|| {
        let Some(running_app) = (unsafe { NSWorkspace::sharedWorkspace().frontmostApplication() })
        else {
            trace("remember_paste_target_window: no frontmost app");
            return;
        };
        let pid = unsafe { running_app.processIdentifier() };
        PASTE_TARGET_PID.store(pid, Ordering::SeqCst);
        trace(&format!("remember_paste_target_window: pid={pid}"));
    });
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn remember_paste_target_window(_app: &AppHandle) {}

/// Read-only accessor for `PASTE_TARGET_PID` — the pid of the app that owned
/// the selection when it was last detected (see `remember_paste_target_window`
/// above). Exposed as a function rather than making the static itself
/// `pub(crate)`, matching this file's existing pattern for state private to
/// `lib.rs` (`remember_paste_target_window`/`focus_paste_target_window`).
/// Used by `selection_watcher`'s mac frontmost-app-switch check to avoid
/// clearing the bubble just because the source app (not reWrite) is
/// frontmost — which is the normal, expected state while the bubble is
/// visible, since `show_bubble`/`show_bubble_menu` deliberately don't steal
/// foreground. 0 means "never set" (no selection captured yet this session).
#[cfg(target_os = "macos")]
pub(crate) fn paste_target_pid() -> i32 {
    PASTE_TARGET_PID.load(Ordering::SeqCst)
}

#[cfg(target_os = "windows")]
pub fn focus_paste_target_window(_app: &AppHandle) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{IsWindow, SetForegroundWindow};

    let hwnd = PASTE_TARGET_HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        trace("focus_paste_target_window: no hwnd stored");
        return;
    }

    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            trace("focus_paste_target_window: hwnd no longer valid");
            return;
        }
        let ok = SetForegroundWindow(hwnd).as_bool();
        trace(&format!("focus_paste_target_window: set_foreground={ok}"));
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn focus_paste_target_window(_app: &AppHandle) {}

/// Same class of bug as `detect_impl`/`remember_paste_target_window`: this is
/// called from the async super-hotkey task (`on_super_hotkey`) and from the
/// async `paste_text` command, neither of which is guaranteed to run on the
/// main thread. Calling AppKit's `NSRunningApplication::activateWithOptions`
/// straight from there would be the exact pattern that crashed
/// `foreground::detect` before that fix. Marshal onto the main thread via
/// `run_on_main_thread`, fire-and-forget — nothing downstream depends on the
/// activation result, only tracing does (mirrors `remember_paste_target_window`).
#[cfg(target_os = "macos")]
pub fn focus_paste_target_window(app: &AppHandle) {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};

    let pid = PASTE_TARGET_PID.load(Ordering::SeqCst);
    if pid == 0 {
        trace("focus_paste_target_window: no pid stored");
        return;
    }

    let _ = app.run_on_main_thread(move || {
        trace(&format!(
            "focus_paste_target_window: on main thread is_main={}",
            is_main_thread()
        ));
        let Some(running_app) =
            (unsafe { NSRunningApplication::runningApplicationWithProcessIdentifier(pid) })
        else {
            trace("focus_paste_target_window: pid no longer valid");
            return;
        };
        let ok = unsafe {
            running_app.activateWithOptions(
                NSApplicationActivationOptions::NSApplicationActivateAllWindows,
            )
        };
        trace(&format!(
            "focus_paste_target_window: activate pid={pid} ok={ok}"
        ));
    });
}

pub struct AppState {
    pub captured_text: Mutex<Option<String>>,
    pub capture_error: Mutex<Option<String>>,
    pub original_clipboard: Mutex<Option<String>>,
    /// Output format chosen from the foreground app at capture time (HTML for
    /// rich-text targets like Outlook/Gmail, plain text otherwise). Sampled
    /// before any of our own windows steal foreground.
    pub foreground_format: Mutex<foreground::OutputFormat>,
    pub config: Mutex<config::Config>,
    pub skills_config: Mutex<skills::SkillsConfig>,
    pub skills_write_lock: Mutex<()>,
    pub history: Mutex<history::HistoryStore>,
    pub http_client: reqwest::Client,
    pub is_capturing: AtomicBool,
    pub is_pasting: AtomicBool,
    /// In-flight guard covering the ENTIRE super-hotkey rewrite
    /// (capture → API → paste), so hammering the super-hotkey cannot fire
    /// overlapping API calls / racing clipboard writes. `is_capturing` only
    /// covers the capture phase; this covers the whole operation.
    pub is_rewriting: AtomicBool,
    pub auth_session: Mutex<Option<auth::AuthSession>>,
    pub subscription: Mutex<auth::SubscriptionCache>,
}

// ── Window helpers ────────────────────────────────────────────────────────────

pub fn show_overlay(app: &AppHandle) {
    // This is called from `on_hotkey`'s spawned async task — i.e. off the main
    // event-loop thread. Win32/WebView2 windows are thread-affine: show/focus/
    // move must run on the thread that owns the window (the main thread). Issuing
    // them from the tokio worker thread is undefined behaviour and crashed the
    // first couple of opens after launch, while the WebView2 controller was
    // still settling from the startup warm pass. Marshal onto the main thread —
    // the same rule `close_overlay` / `open_settings` already follow.
    trace("show_overlay: enter (pre run_on_main_thread)");
    // A real open supersedes the startup warm pass: from here on, the warm-pass
    // hides must not fire (they'd yank the overlay out from under the user).
    OVERLAY_OPENED.store(true, Ordering::SeqCst);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        trace("show_overlay: on main thread");
        if let Some(w) = handle.get_webview_window("overlay") {
            // Re-center in case the window is still parked off-screen from the
            // startup webview-warming pass (see the "Warm the overlay" block).
            trace("show_overlay: center start");
            let _ = w.center();
            trace("show_overlay: show start");
            let _ = w.show();
            trace("show_overlay: set_focus start");
            let _ = w.set_focus();
            trace("show_overlay: window ops done");
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            esc_hook::start(&handle);
            trace("show_overlay: esc_hook::start done");
            return;
        }
        let _ =
            tauri::WebviewWindowBuilder::new(&handle, "overlay", tauri::WebviewUrl::App("".into()))
                .title("")
                .decorations(false)
                .always_on_top(true)
                .transparent(true)
                .skip_taskbar(true)
                .inner_size(480.0, 430.0)
                .center()
                .focused(true)
                .build();
    });
}

pub fn show_processing(app: &AppHandle) {
    let _ = app.emit("processing:show", ());
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = handle.get_webview_window("processing") {
            let _ = w.show();
            return;
        }
        // Fallback if not pre-warmed.
        let _ = tauri::WebviewWindowBuilder::new(
            &handle,
            "processing",
            tauri::WebviewUrl::App("".into()),
        )
        .title("")
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .transparent(true)
        .skip_taskbar(true)
        .inner_size(240.0, 240.0)
        .center()
        .focused(false)
        .build();
    });
}

/// Switch the processing indicator to its "out of free rewrites" state — a red
/// glow — without rebuilding the window. The window is expected to already be
/// visible from a prior `show_processing` call.
pub fn show_processing_limit(app: &AppHandle) {
    let _ = app.emit("processing:limit", ());
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = handle.get_webview_window("processing") {
            let _ = w.show();
        }
    });
}

pub fn hide_processing(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = handle.get_webview_window("processing") {
            let _ = w.hide();
        }
    });
}

/// Clamp `(x, y)` (physical screen coordinates) so `window`, at its current
/// physical size, stays fully within the work area of whichever monitor
/// contains that point. Without this, a multi-monitor setup — or a selection
/// anchor near a monitor's right/bottom edge — can leave the tiny bubble (or
/// the larger bubble menu) partially or fully off-screen. Reads the window's
/// actual current size via the window handle rather than hard-coding the
/// pre-warmed logical sizes, sidestepping any logical/physical DPI-scaling
/// mismatch with the physical `(x, y)` we're clamping.
///
/// Uses `MonitorFromPoint`/`GetMonitorInfoW` from the `windows` crate, same as
/// `selection_watcher.rs`'s `is_foreground_fullscreen_exclusive` (just fed a
/// point instead of a window handle) — no new Cargo.toml dependency needed.
#[cfg(target_os = "macos")]
mod mac_display {
    //! Minimal raw FFI against `CoreGraphics` (already linked transitively via
    //! `enigo` and directly via `esc_hook.rs`/`selection_watcher.rs`, so this
    //! adds no new Cargo.toml dependency) to find which display contains a
    //! point and clamp a rect to that display's bounds — the macOS analogue of
    //! the Windows branch's `MonitorFromPoint`/`GetMonitorInfoW`.
    //!
    //! Coordinate space: `CGDisplayBounds` returns bounds in the same
    //! top-left-origin global "Quartz" display coordinate space that
    //! `CGEventTap`/`CGEventGetLocation` and the macOS Accessibility API
    //! (`AXUIElementCopyElementAtPosition`, `kAXBoundsForRangeParameterizedAttribute`)
    //! use — see the coordinate-space research note in `selection_watcher.rs`'s
    //! `mod mac` for the sourcing. So the `(x, y)` this receives (built from AX
    //! selection bounds in `show_bubble`/`show_bubble_menu`) needs no
    //! conversion before being compared against `CGDisplayBounds`, unlike
    //! AppKit's `NSScreen`/`NSView` bottom-left-origin "flipped" space.
    //!
    //! Uses full display bounds (`CGDisplayBounds`), not a "work area" that
    //! excludes the menu bar/dock — `CoreGraphics` has no direct equivalent of
    //! Win32's `GetMonitorInfoW` work-area rect. This is still fine for
    //! `show_bubble`'s clamp, which just needs to keep the small bubble dot
    //! from hanging off the edge of the screen. `show_bubble_menu`, however,
    //! needs the real visible frame so the menu never gets clamped behind the
    //! Dock — see `work_area_containing` below, which calls AppKit's
    //! `NSScreen.visibleFrame` instead. That's a main-thread-only AppKit call,
    //! but bubble/menu positioning already runs on the main thread via
    //! `run_on_main_thread` elsewhere in this file, so no extra marshaling is
    //! needed.
    type CGDirectDisplayID = u32;
    type CGError = i32;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGSize {
        width: f64,
        height: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGGetDisplaysWithPoint(
            point: CGPoint,
            max_displays: u32,
            displays: *mut CGDirectDisplayID,
            matching_display_count: *mut u32,
        ) -> CGError;
        fn CGDisplayBounds(display: CGDirectDisplayID) -> CGRect;
    }

    /// Pure clamp math, factored out so it's unit-testable without any FFI
    /// call — mirrors the Windows branch's `x.clamp(min_x, max_x)` logic
    /// exactly, just parameterized on the display bounds instead of reading
    /// them from `GetMonitorInfoW` inline.
    pub(super) fn clamp_point_to_bounds(
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        bounds: (f64, f64, f64, f64), // (left, top, right, bottom)
    ) -> (f64, f64) {
        let (left, top, right, bottom) = bounds;
        let min_x = left;
        let max_x = (right - w).max(min_x);
        let min_y = top;
        let max_y = (bottom - h).max(min_y);
        (x.clamp(min_x, max_x), y.clamp(min_y, max_y))
    }

    /// `None` if no display contains `(x, y)` (e.g. already off every screen) —
    /// callers should fall back to the raw unclamped point rather than guess.
    pub(super) fn display_bounds_containing(x: f64, y: f64) -> Option<(f64, f64, f64, f64)> {
        unsafe {
            let point = CGPoint { x, y };
            let mut display: CGDirectDisplayID = 0;
            let mut count: u32 = 0;
            let err = CGGetDisplaysWithPoint(point, 1, &mut display, &mut count);
            if err != 0 || count == 0 {
                return None;
            }
            let bounds = CGDisplayBounds(display);
            Some((
                bounds.origin.x,
                bounds.origin.y,
                bounds.origin.x + bounds.size.width,
                bounds.origin.y + bounds.size.height,
            ))
        }
    }

    /// The visible work area (excludes the menu bar and Dock) of the display
    /// containing `(x, y)`, converted to the same top-left-origin Quartz space
    /// as `display_bounds_containing`/`CGDisplayBounds` above. Unlike that
    /// function, there's no `CoreGraphics` equivalent of a work-area rect, so
    /// this goes through AppKit's `NSScreen.visibleFrame` instead — safe to
    /// call here since `show_bubble_menu` (the only caller) already runs on
    /// the main thread via `run_on_main_thread`.
    ///
    /// AppKit rects are bottom-left-origin ("flipped" relative to Quartz), so
    /// each rect is converted using the primary screen's height `H` (screen
    /// index 0, whose frame origin is always `(0, 0)`): a rect
    /// `(ox, oy, rw, rh)` becomes `(ox, H - (oy + rh), ox + rw, H - oy)`.
    ///
    /// `None` if `MainThreadMarker::new()` fails (not actually on the main
    /// thread) or no screens are reported — callers should fall back to
    /// `display_bounds_containing`'s clamp behavior in that case.
    pub(super) fn work_area_containing(x: f64, y: f64) -> Option<(f64, f64, f64, f64)> {
        use objc2_app_kit::NSScreen;
        use objc2_foundation::{MainThreadMarker, NSRect};

        // SAFETY: the only caller, `show_bubble_menu`, runs this closure inside
        // `run_on_main_thread`, so we are guaranteed to be on the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let screens = NSScreen::screens(mtm);
        // SAFETY: `screens` is AppKit's own array of `NSScreen`, so every
        // element is a valid `NSScreen`; the generic array accessors are only
        // `unsafe` because the element type isn't statically proven.
        let primary_h = unsafe { screens.firstObject() }?.frame().size.height;

        let quartz = |r: NSRect| -> (f64, f64, f64, f64) {
            let (ox, oy, w, h) = (r.origin.x, r.origin.y, r.size.width, r.size.height);
            (ox, primary_h - (oy + h), ox + w, primary_h - oy)
        };

        let screen = (0..screens.count())
            .map(|i| unsafe { screens.objectAtIndex(i) })
            .find(|s| {
                let (l, t, r, b) = quartz(s.frame());
                x >= l && x < r && y >= t && y < b
            })
            .or_else(|| NSScreen::mainScreen(mtm))?;

        Some(quartz(screen.visibleFrame()))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn clamps_inside_bounds_unchanged() {
            let bounds = (0.0, 0.0, 1920.0, 1080.0);
            assert_eq!(
                clamp_point_to_bounds(500.0, 300.0, 168.0, 180.0, bounds),
                (500.0, 300.0)
            );
        }

        #[test]
        fn clamps_past_right_and_bottom_edge() {
            let bounds = (0.0, 0.0, 1920.0, 1080.0);
            assert_eq!(
                clamp_point_to_bounds(1900.0, 1070.0, 168.0, 180.0, bounds),
                (1920.0 - 168.0, 1080.0 - 180.0)
            );
        }

        #[test]
        fn clamps_past_left_and_top_edge() {
            let bounds = (0.0, 0.0, 1920.0, 1080.0);
            assert_eq!(
                clamp_point_to_bounds(-50.0, -20.0, 168.0, 180.0, bounds),
                (0.0, 0.0)
            );
        }

        #[test]
        fn window_larger_than_display_clamps_to_min() {
            // Degenerate case: window wider/taller than the display itself —
            // must not invert min/max (mirrors the Windows branch's
            // `.max(min_x)` guard).
            let bounds = (0.0, 0.0, 100.0, 100.0);
            assert_eq!(
                clamp_point_to_bounds(0.0, 0.0, 500.0, 500.0, bounds),
                (0.0, 0.0)
            );
        }

        #[test]
        fn handles_secondary_display_with_negative_origin() {
            // A display positioned to the left of/above the primary display has
            // negative-origin bounds in Quartz global space; clamp math must
            // still work correctly (not implicitly assume origin is (0, 0)).
            let bounds = (-1920.0, 0.0, 0.0, 1080.0);
            // Fully inside: right edge (-300 + 168 = -132) stays left of the
            // display's right edge (0), so it's untouched.
            assert_eq!(
                clamp_point_to_bounds(-300.0, 300.0, 168.0, 180.0, bounds),
                (-300.0, 300.0)
            );
            // Past the right edge: right edge (-10 + 168 = 158) would cross
            // past 0, so it clamps back to max_x = 0 - 168 = -168.
            assert_eq!(
                clamp_point_to_bounds(-10.0, 300.0, 168.0, 180.0, bounds),
                (-168.0, 300.0)
            );
        }
    }
}

/// `GetMonitorInfoW`'s `rcWork` rect (physical pixels) for the monitor
/// containing `(x, y)`, already excluding the taskbar — factored out of
/// `clamp_rect_to_monitor`'s Windows branch so `show_bubble_menu` can also use
/// it via `work_area_containing_point` below. `None` if no monitor info is
/// available, same "fall back to the raw point" policy as the caller.
#[cfg(target_os = "windows")]
fn win_work_area_containing(x: f64, y: f64) -> Option<(f64, f64, f64, f64)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };

    let pt = POINT {
        x: x as i32,
        y: y as i32,
    };
    let hmonitor = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };

    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(hmonitor, &mut info) }
        .ok()
        .is_err()
    {
        return None;
    }

    let work = info.rcWork;
    Some((
        work.left as f64,
        work.top as f64,
        work.right as f64,
        work.bottom as f64,
    ))
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn clamp_rect_to_monitor(x: f64, y: f64, w: f64, h: f64) -> (f64, f64) {
    #[cfg(target_os = "macos")]
    {
        return match mac_display::display_bounds_containing(x, y) {
            Some(bounds) => mac_display::clamp_point_to_bounds(x, y, w, h, bounds),
            // No display contains the raw point (e.g. it's already off every
            // screen) — fall back to the raw point rather than guess, same
            // policy as the Windows branch's "no monitor info" fallback below.
            None => (x, y),
        };
    }

    #[cfg(target_os = "windows")]
    {
        let Some((left, top, right, bottom)) = win_work_area_containing(x, y) else {
            // No monitor info available — fall back to the raw, unclamped
            // point rather than guessing.
            return (x, y);
        };
        let min_x = left;
        let max_x = (right - w).max(min_x);
        let min_y = top;
        let max_y = (bottom - h).max(min_y);

        (x.clamp(min_x, max_x), y.clamp(min_y, max_y))
    }
}

/// Places a `w`x`h` menu relative to anchor `(ax, ay)` inside work area
/// `(left, top, right, bottom)`. Horizontally prefers the anchor's left edge,
/// flipping left near the right edge. Vertically: when the bubble sits in the
/// bottom `UPWARD_ZONE` of the work area the menu opens *upward* — its
/// bottom-left corner anchored at the bubble — so it's never clipped by the
/// Dock/taskbar; otherwise it opens downward from the bubble's top-left. A
/// final clamp keeps it fully on-screen.
fn place_menu(ax: f64, ay: f64, w: f64, h: f64, work: (f64, f64, f64, f64)) -> (f64, f64) {
    const GAP: f64 = 8.0;
    const UPWARD_ZONE: f64 = 0.30;
    let (left, top, right, bottom) = work;
    let x = if ax + GAP + w <= right { ax + GAP } else { ax - w };
    let opens_upward = ay >= bottom - UPWARD_ZONE * (bottom - top);
    let y = if opens_upward { ay - h } else { ay + GAP };
    let max_x = (right - w).max(left);
    let max_y = (bottom - h).max(top);
    (x.clamp(left, max_x), y.clamp(top, max_y))
}

#[cfg(test)]
mod place_menu_tests {
    use super::*;

    const WORK: (f64, f64, f64, f64) = (0.0, 0.0, 1920.0, 1080.0 - 60.0); // Dock excluded

    #[test]
    fn fits_below_right_unchanged() {
        // Plenty of room below and to the right of the anchor: no flip.
        assert_eq!(
            place_menu(500.0, 300.0, 168.0, 180.0, WORK),
            (500.0 + 8.0, 300.0 + 8.0)
        );
    }

    #[test]
    fn flips_above_near_bottom() {
        // Anchor close to the bottom of the work area (not the full display)
        // — the menu must open upward (bottom-left corner at the bubble), not
        // get clamped under the Dock.
        let (_, _, _, bottom) = WORK;
        let ay = bottom - 20.0;
        let (_, y) = place_menu(500.0, ay, 168.0, 180.0, WORK);
        assert_eq!(y, ay - 180.0);
        assert!(y + 180.0 <= bottom, "menu must stay within the work area");
    }

    #[test]
    fn opens_upward_in_bottom_zone_even_when_it_would_fit() {
        // Bubble in the bottom 30% of the work area: the menu opens upward
        // (bottom-left corner at the bubble) even though it would still fit
        // below — the whole point of the zone rule.
        let (_, top, _, bottom) = WORK;
        // 25% up: inside the bottom-30% zone, yet with room to fit below too.
        let ay = bottom - 0.25 * (bottom - top);
        assert!(ay >= bottom - 0.30 * (bottom - top), "precondition: in the zone");
        assert!(ay + 8.0 + 180.0 <= bottom, "precondition: it would fit below");
        let (_, y) = place_menu(500.0, ay, 168.0, 180.0, WORK);
        assert_eq!(y, ay - 180.0, "should anchor bottom-left at the bubble");
    }

    #[test]
    fn opens_downward_just_above_the_zone() {
        // Just above the bottom-30% boundary: still opens downward.
        let (_, top, _, bottom) = WORK;
        let ay = bottom - 0.35 * (bottom - top);
        let (_, y) = place_menu(500.0, ay, 168.0, 180.0, WORK);
        assert_eq!(y, ay + 8.0);
    }

    #[test]
    fn flips_left_near_right_edge() {
        // Anchor close to the right edge — the menu must flip to the left of
        // the anchor rather than clamp/overlap it.
        let (_, _, right, _) = WORK;
        let ax = right - 20.0;
        let (x, _) = place_menu(ax, 300.0, 168.0, 180.0, WORK);
        assert_eq!(x, ax - 168.0);
        assert!(x + 168.0 <= right, "menu must stay within the work area");
    }
}

/// A window's outer size converted to the unit the platform's monitor/work-
/// area math expects: logical points on macOS (matching `CGDisplayBounds`/
/// `NSScreen` and the incoming points-space `x, y`), physical pixels on
/// Windows (matching `GetMonitorInfoW`). Factored out of `clamp_to_monitor` so
/// `show_bubble_menu` can also feed a window's real size into `place_menu`.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn menu_logical_size(window: &WebviewWindow) -> (f64, f64) {
    #[cfg(target_os = "macos")]
    {
        // `window.outer_size()` returns Tauri's `PhysicalSize` (device
        // pixels) regardless of platform, so using it directly here would mix
        // pixel-space `w`/`h` into a point-space clamp — on any Retina
        // display (scale factor 2, the overwhelming majority of real Macs)
        // that over-subtracts near the display's right/bottom edge by
        // roughly 2x the window's true point-space size. Convert to
        // logical/points via the window's own scale factor first.
        let scale = window.scale_factor().unwrap_or(1.0);
        return window
            .outer_size()
            .map(|s| {
                let logical = s.to_logical::<f64>(scale);
                (logical.width, logical.height)
            })
            .unwrap_or((0.0, 0.0));
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: UIA selection rects and `GetWindowRect`/monitor info are
        // already all in physical pixels, matching `outer_size()` directly —
        // no conversion needed here.
        window
            .outer_size()
            .map(|s| (s.width as f64, s.height as f64))
            .unwrap_or((0.0, 0.0))
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn menu_logical_size(_window: &WebviewWindow) -> (f64, f64) {
    (0.0, 0.0)
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn clamp_to_monitor(window: &WebviewWindow, x: f64, y: f64) -> (f64, f64) {
    let (w, h) = menu_logical_size(window);
    clamp_rect_to_monitor(x, y, w, h)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn clamp_rect_to_monitor(x: f64, y: f64, _w: f64, _h: f64) -> (f64, f64) {
    (x, y)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn clamp_to_monitor(_window: &WebviewWindow, x: f64, y: f64) -> (f64, f64) {
    (x, y)
}

/// The visible work area (excluding the Dock/taskbar and menu bar) containing
/// `(x, y)`, in the same per-platform coordinate space `clamp_to_monitor`
/// uses — see `mac_display::work_area_containing` and
/// `win_work_area_containing`. `None` on platforms without an implementation,
/// or if the underlying lookup fails, so callers can fall back to
/// `clamp_to_monitor`'s plain-clamp behavior.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn work_area_containing_point(x: f64, y: f64) -> Option<(f64, f64, f64, f64)> {
    #[cfg(target_os = "macos")]
    {
        return mac_display::work_area_containing(x, y);
    }

    #[cfg(target_os = "windows")]
    {
        win_work_area_containing(x, y)
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn work_area_containing_point(_x: f64, _y: f64) -> Option<(f64, f64, f64, f64)> {
    None
}

/// Show the selection bubble near `(x, y)` (physical screen coordinates from
/// the `selection:detected` event payload). Marshaled onto the main thread
/// like every other window show/hide/position call in this file — see the
/// comment on `show_overlay` for why that's not optional.
pub fn show_bubble(app: &AppHandle, x: f64, y: f64) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = handle.get_webview_window("bubble") {
            // The window is deliberately larger than the visible 16x16 ring
            // (see BUBBLE_HIT_PADDING / Bubble.tsx) so the actual clickable
            // area is more forgiving than the tiny visible dot — a live trace
            // session showed clicks aimed at the old 16x16 target routinely
            // missing it entirely (the click landed on the source app instead,
            // which silently cleared the selection, and `bubble_clicked` never
            // fired). UIA gives us the bottom-right corner of the final
            // selection rect. Place the visible ring's center very close to
            // that anchor, then subtract the ring radius and transparent
            // padding because set_position moves the larger hit window, not
            // the visible ring itself.
            let (x, y) = clamp_to_monitor(
                &w,
                x + BUBBLE_ANCHOR_CENTER_OFFSET_X - BUBBLE_VISIBLE_SIZE / 2.0 - BUBBLE_HIT_PADDING,
                y + BUBBLE_ANCHOR_CENTER_OFFSET_Y - BUBBLE_VISIBLE_SIZE / 2.0 - BUBBLE_HIT_PADDING,
            );
            // `(x, y)` here is in physical pixels on Windows (UIA's native
            // unit) but points on macOS (AX/CGEventTap's native unit, which
            // is exactly Tauri's "logical" position there) — see
            // `clamp_to_monitor`'s doc comment. Using `PhysicalPosition` on
            // macOS would place the bubble at roughly half the intended
            // position on any Retina display.
            #[cfg(target_os = "macos")]
            let _ = w.set_position(LogicalPosition::new(x, y));
            #[cfg(not(target_os = "macos"))]
            let _ = w.set_position(PhysicalPosition::new(x, y));
            // Re-toggling always-on-top moves the window to the very top of
            // the topmost band, ahead of any other always-on-top window (some
            // chat/call apps run topmost themselves) that could otherwise
            // render over our bubble and make it appear not to respond to
            // clicks. See the same trick in show_bubble_menu.
            let _ = w.set_always_on_top(false);
            let _ = w.set_always_on_top(true);
            let show_result = w.show();
            trace(&format!(
                "show_bubble: at ({x}, {y}) show={:?}",
                show_result.is_ok()
            ));
        } else {
            trace("show_bubble: bubble window not found");
        }
    });
}

pub fn hide_bubble(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = handle.get_webview_window("bubble") {
            let _ = w.hide();
            trace("hide_bubble: hidden");
        }
    });
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn bubble_menu_probe_suppressed() -> bool {
    now_ms() < BUBBLE_MENU_SUPPRESS_PROBE_UNTIL_MS.load(Ordering::SeqCst)
}

fn suppress_bubble_menu_probe() {
    BUBBLE_MENU_SUPPRESS_PROBE_UNTIL_MS
        .store(now_ms() + BUBBLE_MENU_CLOSE_SUPPRESS_MS, Ordering::SeqCst);
}

fn suppress_bubble_menu_outside_click() {
    BUBBLE_MENU_IGNORE_OUTSIDE_CLICK_UNTIL_MS
        .store(now_ms() + BUBBLE_MENU_OPEN_CLICK_GRACE_MS, Ordering::SeqCst);
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn bubble_menu_outside_click_suppressed() -> bool {
    now_ms() < BUBBLE_MENU_IGNORE_OUTSIDE_CLICK_UNTIL_MS.load(Ordering::SeqCst)
}

/// Show the bubble's skill menu near `(x, y)`, offset slightly so it doesn't
/// sit exactly on top of the bubble icon that was just clicked.
///
/// The menu webview is prebuilt at startup. Building or reloading a WebView
/// from this click path can stall or briefly paint transparent on Windows, so
/// showing means moving the existing window on-screen, focusing it, then
/// resetting the live React tree by event.
pub fn show_bubble_menu(app: &AppHandle, x: f64, y: f64) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        trace("show_bubble_menu: on main thread");
        // The global mouse hook sees the same WM_LBUTTONUP that clicked the
        // bubble. Depending on main-thread queue ordering, its outside-click
        // task can run just after this fresh menu is built and otherwise
        // destroy it as an "outside" click before the user ever sees it.
        suppress_bubble_menu_outside_click();

        if let Some(w) = handle.get_webview_window("bubble_menu") {
            let _ = w.set_size(LogicalSize::new(BUBBLE_MENU_WIDTH, BUBBLE_MENU_HEIGHT));
            // Size using the window's own `outer_size()` (via
            // `menu_logical_size`) rather than the logical BUBBLE_MENU_WIDTH/
            // HEIGHT constants directly. On Windows, the work-area math below
            // subtracts w/h from `rcWork`, which `GetMonitorInfoW` reports in
            // *physical* pixels — feeding it the logical constants there
            // under-subtracts by the monitor's scale factor on any HiDPI
            // display, letting the menu hang off the right/bottom edge.
            // `menu_logical_size` reads `outer_size()`, which is already in
            // physical pixels on Windows (no conversion needed) and is
            // converted to points on macOS (matching `CGDisplayBounds`/
            // `NSScreen` and the incoming points-space `x, y`) — see its doc
            // comment. The `set_size` call above runs first so `outer_size()`
            // reflects the menu's real dimensions before this reads it.
            //
            // Prefer the real visible-work-area lookup (excludes the Dock/
            // menu bar/taskbar) so the menu can flip above/left of the anchor
            // instead of getting clamped behind the Dock near screen edges —
            // see `place_menu`'s doc comment. Only fall back to the plain
            // clamp-to-full-display behavior if that lookup fails.
            let (menu_w, menu_h) = menu_logical_size(&w);
            let (x, y) = match work_area_containing_point(x, y) {
                Some(work) => place_menu(x, y, menu_w, menu_h, work),
                None => clamp_to_monitor(&w, x + 8.0, y + 8.0),
            };
            // Same points-vs-physical-pixels distinction as `show_bubble` —
            // see that call site's comment.
            #[cfg(target_os = "macos")]
            let _ = w.set_position(LogicalPosition::new(x, y));
            #[cfg(not(target_os = "macos"))]
            let _ = w.set_position(PhysicalPosition::new(x, y));
            // Reload the webview *now that it is back on-screen*, not at close
            // time. `hide_bubble_menu` parks this window at (-32000, -32000);
            // reloading it there (as it used to) runs the fresh page load under
            // WebView2's off-screen occlusion throttling, so the new tree often
            // never paints — the window then reveals a blank surface on the next
            // open (see bubble-menu-bug-diagnosis.md, "cross-cutting lesson").
            // Doing the reload here, while the window is on a real monitor,
            // forces a fresh React tree that actually renders, and makes any
            // stale error/loading state from the previous use impossible.
            let reload_result = w.reload();
            let _ = w.set_always_on_top(false);
            let _ = w.set_always_on_top(true);
            let show_result = w.show();
            let focus_result = w.set_focus();
            trace(&format!(
                "show_bubble_menu: shown at ({x}, {y}) reload={:?} show={:?} focus={:?}",
                reload_result.is_ok(),
                show_result.is_ok(),
                focus_result.is_ok(),
            ));
        } else {
            trace("show_bubble_menu: bubble_menu window not found");
        }
    });
}

pub fn hide_bubble_menu(app: &AppHandle) {
    hide_bubble_menu_inner(app, true);
}

fn hide_bubble_menu_inner(app: &AppHandle, suppress_probe: bool) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if suppress_probe {
            suppress_bubble_menu_probe();
        }
        if let Some(w) = handle.get_webview_window("bubble_menu") {
            // Reset is emitted while the window is still on-screen (its JS is
            // reliably live at this instant), then it is parked off-screen. The
            // fresh-page reload deliberately does NOT happen here: reloading a
            // parked/off-screen window runs under WebView2 occlusion throttling
            // and often fails to paint. `show_bubble_menu` reloads instead, once
            // the window is back on a real monitor. See bubble-menu-bug-diagnosis.md.
            let _ = w.emit("bubble-menu:reset", ());
            let _ = w.set_position(PhysicalPosition::new(
                BUBBLE_MENU_PARKED_X,
                BUBBLE_MENU_PARKED_Y,
            ));
            trace(&format!(
                "hide_bubble_menu: reset emitted, parked window, suppress_probe={suppress_probe}"
            ));
        }
    });
}

/// If the bubble menu is currently open and `(x, y)` (physical screen coords
/// of a click, from the low-level mouse hook in `selection_watcher.rs`) falls
/// outside its window bounds, closes it. This is the primary "click outside
/// closes the menu" mechanism — driven directly off the same global mouse
/// hook that already handles selection detection, rather than the WebView2/
/// WKWebView focus/blur events `BubbleMenu.tsx` also listens for, since those
/// have proven unreliable cross-app (a window's OS focus state can lag or
/// fail to transfer depending on how it was shown — a Windows-observed issue,
/// but the same class of race is plausible on macOS too, so the mac backend
/// uses this same mouse-hook-driven mechanism rather than trusting WKWebView
/// blur). All window queries run on the main thread, like every other window
/// operation in this file. Body is pure cross-platform Tauri window API calls
/// (`get_webview_window`/`outer_position`/`outer_size`/`run_on_main_thread`) —
/// nothing Windows-specific — so this is shared by both hook backends.
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub fn maybe_close_bubble_menu_on_outside_click(
    app: &AppHandle,
    x: i32,
    y: i32,
    allow_probe_after_close: bool,
) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(w) = handle.get_webview_window("bubble_menu") else {
            return;
        };
        let Ok(cur_pos) = w.outer_position() else {
            return;
        };
        if is_bubble_menu_parked(cur_pos.x, cur_pos.y) {
            return;
        }
        if bubble_menu_outside_click_suppressed() {
            trace("maybe_close_bubble_menu_on_outside_click: skipped, opening click grace");
            return;
        }
        let inside = match w.outer_size() {
            Ok(size) => {
                // Same points-vs-physical-pixels distinction as
                // `clamp_to_monitor`/`show_bubble`: on macOS `x, y` arrive in
                // points (from the mac tap callback's `CGEventGetLocation`),
                // but `cur_pos`/`size` are Tauri's `PhysicalPosition`/
                // `PhysicalSize` (device pixels) on every platform. Comparing
                // them directly on a Retina Mac would make this hit-test
                // wrong by roughly 2x — a genuine outside click could still
                // numerically land "inside" the (larger, pixel-space) window
                // rect, so the bubble menu would fail to close on some
                // outside clicks. Convert the window's bounds down to
                // logical/points on macOS before comparing; Windows keeps
                // comparing physical-to-physical directly, since UIA/
                // `GetWindowRect` are already both physical there.
                #[cfg(target_os = "macos")]
                {
                    let scale = w.scale_factor().unwrap_or(1.0);
                    let pos = cur_pos.to_logical::<f64>(scale);
                    let size = size.to_logical::<f64>(scale);
                    let (x, y) = (x as f64, y as f64);
                    x >= pos.x && x <= pos.x + size.width && y >= pos.y && y <= pos.y + size.height
                }
                #[cfg(not(target_os = "macos"))]
                {
                    x >= cur_pos.x
                        && x <= cur_pos.x + size.width as i32
                        && y >= cur_pos.y
                        && y <= cur_pos.y + size.height as i32
                }
            }
            // Bounds unknowable — don't close on a guess.
            Err(_) => true,
        };
        if !inside {
            trace("maybe_close_bubble_menu_on_outside_click: click outside, closing");
            // Delegate to hide_bubble_menu so every close parks/resets the
            // webview and arms the same short re-probe suppression.
            hide_bubble_menu_inner(&handle, !allow_probe_after_close);
        }
    });
}

/// Whether a rewrite error corresponds to the user exhausting their free
/// rewrites / subscription limit (HTTP 402 codes surfaced by `call_api_raw`).
fn is_limit_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("limit") || m.contains("trial") || m.contains("quota") || m.contains("upgrade")
}

/// Return a currently-valid Supabase access token, refreshing it on demand when
/// the stored one has expired (or is within `is_expired`'s skew window).
///
/// Supabase access tokens live ~1 hour. We previously refreshed only at startup,
/// so a session left running past that hour sent an expired JWT on every
/// authenticated call and the Edge Function replied `401 Invalid JWT` until the
/// app was restarted. Routing every authenticated call through this helper keeps
/// the token fresh transparently. A refreshed session is persisted to disk and
/// state; a failed refresh clears the session so the UI can prompt a re-login.
///
/// Care: we snapshot the session and drop every lock/`State` guard before the
/// `.await`, then re-acquire state afterwards — never holding a lock across it.
pub async fn ensure_valid_token(app: &AppHandle) -> Option<String> {
    let (session, client) = {
        let state = app.try_state::<AppState>()?;
        let session = state.auth_session.lock().unwrap().as_ref().cloned()?;
        (session, state.http_client.clone())
    };

    if !auth::is_expired(&session) {
        return Some(session.access_token);
    }

    let auth_path = app
        .path()
        .app_config_dir()
        .ok()
        .map(|d| d.join("auth.json"));

    match auth::refresh_session(&client, session).await {
        Ok(refreshed) => {
            if let Some(ref path) = auth_path {
                let _ = auth::save_session(&refreshed, path);
            }
            let token = refreshed.access_token.clone();
            if let Some(state) = app.try_state::<AppState>() {
                *state.auth_session.lock().unwrap() = Some(refreshed);
            }
            Some(token)
        }
        Err(_) => {
            if let Some(ref path) = auth_path {
                auth::clear_session(path);
            }
            if let Some(state) = app.try_state::<AppState>() {
                *state.auth_session.lock().unwrap() = None;
            }
            None
        }
    }
}

/// Persists a freshly-synced subscription cache to disk, best-effort, so a
/// crash or relaunch before the *next* sync still shows the last-known-good
/// plan instead of quietly reverting to Free. Mirrors the auth.json write
/// path used for `AuthSession` elsewhere in this file.
pub(crate) fn persist_subscription(app: &AppHandle, sub: &auth::SubscriptionCache) {
    if let Ok(path) = app
        .path()
        .app_config_dir()
        .map(|d| d.join("subscription.json"))
    {
        let _ = auth::save_subscription(sub, &path);
    }
}

static LAST_OPEN_SYNC_MS: AtomicU64 = AtomicU64::new(0);
const OPEN_SYNC_THROTTLE_MS: u64 = 20_000;

/// Best-effort cloud pull when the dashboard is opened, so an already-running
/// app reflects skill/history edits made on other devices without a restart
/// (startup sync alone would leave it stale until relaunch). Throttled, and a
/// no-op when logged out. `sync_all` emits `skills:updated`/`history:updated`,
/// which the frontend already listens for and re-reads on.
fn sync_on_open(app: &AppHandle) {
    let logged_in = app
        .try_state::<AppState>()
        .is_some_and(|s| s.auth_session.lock().unwrap().is_some());
    let now = now_ms();
    if !logged_in || now.saturating_sub(LAST_OPEN_SYNC_MS.load(Ordering::SeqCst)) < OPEN_SYNC_THROTTLE_MS {
        return;
    }
    LAST_OPEN_SYNC_MS.store(now, Ordering::SeqCst);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = sync::sync_all(&app).await {
            trace(&format!("cloud sync on open failed: {error}"));
        }
    });
}

pub fn show_settings(app: &AppHandle) {
    // The Settings window is pre-warmed (hidden) at startup, so opening it is
    // just a show + focus — we never build a webview at runtime here. Building
    // a second webview from a command/menu callback on Windows can deadlock the
    // main event loop (the new window paints blank and the app freezes), so the
    // window must already exist. The build path below is only a safety net for
    // the unlikely case that pre-warming failed.
    sync_on_open(app);
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    if let Ok(w) =
        tauri::WebviewWindowBuilder::new(app, "settings", tauri::WebviewUrl::App("".into()))
            .title("reWrite - Settings")
            .decorations(true)
            .always_on_top(false)
            .inner_size(1260.0, 870.0)
            .min_inner_size(900.0, 600.0)
            .center()
            .resizable(true)
            .build()
    {
        let _ = w.set_focus();
    }
}

// ── Deep-link handler ─────────────────────────────────────────────────────────

fn handle_deep_link(app: &AppHandle, url: &str) {
    if url.starts_with("rewrite://checkout-success") {
        show_settings(app);

        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let Some(state) = app.try_state::<AppState>() else {
                return;
            };
            let access_token = state
                .auth_session
                .lock()
                .unwrap()
                .as_ref()
                .map(|s| s.access_token.clone());
            let Some(access_token) = access_token else {
                return;
            };

            if let Ok(sub) = auth::sync_subscription(&state.http_client, &access_token).await {
                persist_subscription(&app, &sub);
                *state.subscription.lock().unwrap() = sub;
            }

            let _ = app.emit("auth:complete", ());
        });
        return;
    }

    if url.starts_with("rewrite://checkout-cancelled") {
        show_settings(app);
        return;
    }

    // Returning from the Stripe billing portal — the user may have changed
    // plan or cancelled, so re-sync subscription state.
    if url.starts_with("rewrite://portal-return") {
        show_settings(app);

        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let Some(state) = app.try_state::<AppState>() else {
                return;
            };
            let access_token = state
                .auth_session
                .lock()
                .unwrap()
                .as_ref()
                .map(|s| s.access_token.clone());
            let Some(access_token) = access_token else {
                return;
            };

            if let Ok(sub) = auth::sync_subscription(&state.http_client, &access_token).await {
                persist_subscription(&app, &sub);
                *state.subscription.lock().unwrap() = sub;
            }

            let _ = app.emit("auth:complete", ());
        });
        return;
    }

    if !url.starts_with("rewrite://auth") {
        return;
    }

    let Some((access_token, refresh_token, expires_at)) = auth::parse_auth_url(url) else {
        return;
    };

    // Bring the settings window forward so the user lands back in the app
    // (already signed in) after the browser hands off the tokens.
    show_settings(app);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let client = state.http_client.clone();

        let user = auth::get_user(&client, &access_token).await.ok();

        let session = auth::AuthSession {
            user_id: user.as_ref().map(|u| u.id.clone()).unwrap_or_default(),
            access_token: access_token.clone(),
            refresh_token,
            expires_at,
            name: user.as_ref().map(|u| u.display_name()).unwrap_or_default(),
            email: user.and_then(|u| u.email).unwrap_or_default(),
        };

        if let Ok(path) = app.path().app_config_dir().map(|d| d.join("auth.json")) {
            let _ = auth::save_session(&session, &path);
        }

        *state.auth_session.lock().unwrap() = Some(session.clone());
        // Let the login view transition immediately; subscription and cloud
        // reconciliation remain background/best-effort and emit their own
        // refresh events as data arrives.
        let _ = app.emit("auth:complete", ());
        let sync_app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = sync::sync_all(&sync_app).await {
                trace(&format!("cloud sync after login failed: {error}"));
            }
        });

        if let Ok(sub) = auth::sync_subscription(&client, &session.access_token).await {
            persist_subscription(&app, &sub);
            *state.subscription.lock().unwrap() = sub;
        }
    });
}

// ── Hotkey handlers ───────────────────────────────────────────────────────────

fn on_hotkey(app: &AppHandle) {
    // Diagnostic for the macOS main-thread question — see `MAIN_THREAD_ID`.
    // `trace()` already logs `tid=` on every line; `is_main` here is the
    // explicit comparison against the thread `run()` started on.
    trace(&format!("on_hotkey: enter is_main={}", is_main_thread()));
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };

    // Sample the foreground app now, while the target still has focus — before
    // the overlay steals it — so we know whether to emit HTML or plain text.
    trace("on_hotkey: foreground::detect start");
    let detected_fmt = foreground::detect(app);
    if let Ok(mut fmt) = state.foreground_format.lock() {
        *fmt = detected_fmt;
    }
    trace("on_hotkey: foreground::detect done");

    if state.is_capturing.swap(true, Ordering::SeqCst) {
        trace("on_hotkey: already capturing, bail");
        return;
    }

    remember_paste_target_window(app);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        trace("on_hotkey: capture_selection start");
        let result = tokio::task::spawn_blocking(clipboard::capture_selection).await;
        trace("on_hotkey: capture_selection done");

        if let Some(s) = app.try_state::<AppState>() {
            s.is_capturing.store(false, Ordering::SeqCst);

            let (text_val, orig_val, err_val) = match result {
                Ok(Ok((text, original))) => {
                    let err = text.is_empty().then(|| {
                        "No text captured. Make sure text is still highlighted and reWrite has macOS Accessibility permission.".to_string()
                    });
                    (
                        (!text.is_empty()).then_some(text),
                        (!original.is_empty()).then_some(original),
                        err,
                    )
                }
                Ok(Err(e)) => (None, None, Some(e.to_string())),
                Err(e) => (
                    None,
                    None,
                    Some(format!("Selection capture task failed: {e}")),
                ),
            };

            if let Ok(mut g) = s.captured_text.lock() {
                *g = text_val;
            }
            if let Ok(mut g) = s.capture_error.lock() {
                *g = err_val;
            }
            if let Ok(mut g) = s.original_clipboard.lock() {
                *g = orig_val;
            }
        }

        show_overlay(&app);
    });
}

/// RAII guard that clears `AppState::is_rewriting` on drop, guaranteeing the
/// whole-rewrite in-flight flag is released on EVERY exit path of the
/// super-hotkey async task (early returns and the success path alike).
struct RewriteGuard {
    app: AppHandle,
}

impl Drop for RewriteGuard {
    fn drop(&mut self) {
        if let Some(state) = self.app.try_state::<AppState>() {
            state.is_rewriting.store(false, Ordering::SeqCst);
        }
    }
}

fn on_super_hotkey(app: &AppHandle) {
    // Diagnostic for the macOS main-thread question — see `MAIN_THREAD_ID`.
    trace(&format!(
        "on_super_hotkey: enter is_main={}",
        is_main_thread()
    ));
    let Some(state) = app.try_state::<AppState>() else {
        trace("on_super_hotkey: no AppState");
        return;
    };

    // Whole-rewrite in-flight guard: a second concurrent super-hotkey press
    // while a rewrite is running is dropped silently. Set this BEFORE
    // `show_processing` so the second press shows nothing.
    if state.is_rewriting.swap(true, Ordering::SeqCst) {
        trace("on_super_hotkey: already rewriting, bail");
        return;
    }

    if state.is_capturing.swap(true, Ordering::SeqCst) {
        // A capture (from either hotkey) is already in flight; release the
        // rewrite reservation we just took before bailing.
        trace("on_super_hotkey: already capturing, bail");
        state.is_rewriting.store(false, Ordering::SeqCst);
        return;
    }

    // Sample the foreground app now — after the guards (so a dropped duplicate
    // press can't overwrite the in-flight rewrite's format) but before
    // `show_processing` below steals focus, so the decision reflects the user's
    // real target app.
    trace("on_super_hotkey: foreground::detect start");
    let detected_fmt = foreground::detect(app);
    if let Ok(mut fmt) = state.foreground_format.lock() {
        *fmt = detected_fmt;
    }
    trace("on_super_hotkey: foreground::detect done");

    remember_paste_target_window(app);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Resets `is_rewriting` on drop — covers ALL returns below plus the
        // success path at the end of this async block.
        let _rewrite_guard = RewriteGuard { app: app.clone() };

        trace("on_super_hotkey: capture_selection start");
        let capture_result = tokio::task::spawn_blocking(clipboard::capture_selection).await;
        trace("on_super_hotkey: capture_selection done");

        let Some(state) = app.try_state::<AppState>() else {
            trace("on_super_hotkey: no AppState after capture");
            hide_processing(&app);
            return;
        };
        state.is_capturing.store(false, Ordering::SeqCst);

        // Require auth — refresh the token on demand if it has expired, so a
        // long-running session doesn't send a stale JWT and get a 401.
        let Some(access_token) = ensure_valid_token(&app).await else {
            trace("on_super_hotkey: no valid token");
            hide_processing(&app);
            return;
        };

        let (text, original) = match capture_result {
            Ok(Ok((t, o))) if !t.is_empty() => (t, o),
            Ok(Ok((_t, _o))) => {
                trace("on_super_hotkey: empty capture");
                return;
            }
            Ok(Err(e)) => {
                trace(&format!("on_super_hotkey: capture error: {e}"));
                return;
            }
            Err(e) => {
                trace(&format!("on_super_hotkey: capture task failed: {e}"));
                return;
            }
        };

        trace("on_super_hotkey: show_processing start");
        show_processing(&app);
        trace("on_super_hotkey: show_processing done");

        if let Ok(mut g) = state.captured_text.lock() {
            *g = Some(text.clone());
        }
        if let Ok(mut g) = state.original_clipboard.lock() {
            *g = Some(original.clone());
        }

        let (model, default_skill_id, paste_delay_ms, restore, restore_delay_ms) = {
            let Ok(cfg) = state.config.lock() else {
                hide_processing(&app);
                return;
            };
            (
                cfg.model.clone(),
                cfg.default_skill_id.clone(),
                cfg.paste_delay_ms,
                cfg.restore_clipboard,
                cfg.restore_delay_ms,
            )
        };

        let format = state
            .foreground_format
            .lock()
            .map(|f| *f)
            .unwrap_or_default();

        let (system, skill_name) = {
            let Ok(sc) = state.skills_config.lock() else {
                hide_processing(&app);
                return;
            };
            let system = skills::build_system_prompt(&sc, Some(&default_skill_id), format);
            let name = skills::skill_display_name(&sc, &default_skill_id);
            (system, name)
        };

        let client = state.http_client.clone();
        let user_message = format!("<text>\n{text}\n</text>");

        let result =
            match rewrite::call_api_raw(&client, &access_token, &system, &user_message, &model)
                .await
            {
                Ok(o) => o,
                Err(e) => {
                    trace(&format!("on_super_hotkey: rewrite API error: {e}"));
                    if is_limit_error(&e.to_string()) {
                        // Show the red "out of free rewrites" glow briefly, then dismiss.
                        show_processing_limit(&app);
                        tokio::time::sleep(tokio::time::Duration::from_millis(2200)).await;
                    }
                    hide_processing(&app);
                    return;
                }
            };
        let output = result.text;
        // For HTML targets the model returns markup; keep a plain-text form for
        // the clipboard fallback and for history / word-count.
        let plain_output = match format {
            foreground::OutputFormat::Html => clipboard::strip_html_tags(&output),
            foreground::OutputFormat::PlainText => output.clone(),
        };

        // Keep the local usage cache in step with the server-side count so the
        // Settings usage figure reflects super-hotkey rewrites too.
        if let Some(count) = result.rewrite_count {
            if let Ok(mut sub) = state.subscription.lock() {
                sub.rewrite_count = count;
            }
            let _ = app.emit("usage:updated", ());
        }

        let entry = history::HistoryEntry {
            id: skills::new_id(),
            timestamp_ms: history::now_ms(),
            skill_id: default_skill_id,
            skill_name,
            input_text: text,
            output_text: plain_output.clone(),
            output_word_count: history::count_words(&plain_output),
        };
        if let Err(error) = history::append_and_sync(&app, &state, entry) {
            trace(&format!("history append failed: {error}"));
        }

        focus_paste_target_window(&app);
        let paste_trace_id = next_paste_trace_id();
        trace(&format!(
            "paste#{paste_trace_id}: super-hotkey paste scheduled format={format:?} output={} original_len={} restore={} restore_delay_ms={} paste_delay_ms={}",
            text_fingerprint(&output),
            original.len(),
            restore,
            restore_delay_ms,
            paste_delay_ms
        ));
        tokio::time::sleep(tokio::time::Duration::from_millis(paste_delay_ms)).await;
        let _ = tokio::task::spawn_blocking(move || match format {
            foreground::OutputFormat::Html => clipboard::paste_html_and_restore(
                paste_trace_id,
                &output,
                &plain_output,
                &original,
                restore,
                restore_delay_ms,
            ),
            foreground::OutputFormat::PlainText => clipboard::paste_and_restore(
                paste_trace_id,
                &output,
                &original,
                restore,
                restore_delay_ms,
            ),
        })
        .await;

        hide_processing(&app);
    });
}

// ── App entry point ───────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Capture "the main thread" before anything else runs. See
    // `MAIN_THREAD_ID` for why.
    let _ = MAIN_THREAD_ID.set(std::thread::current().id());

    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_single_instance::init(|_app, _argv, _cwd| {}))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }

                    let Some(state) = app.try_state::<AppState>() else {
                        return;
                    };
                    let Ok(cfg) = state.config.lock() else { return };
                    let super_hk = cfg.super_hotkey.clone();
                    drop(cfg);

                    let is_super = super_hk
                        .parse::<tauri_plugin_global_shortcut::Shortcut>()
                        .map(|sc| shortcut == &sc)
                        .unwrap_or(false);

                    if is_super {
                        on_super_hotkey(app);
                    } else {
                        on_hotkey(app);
                    }
                })
                .build(),
        )
        .manage(AppState {
            captured_text: Mutex::new(None),
            capture_error: Mutex::new(None),
            original_clipboard: Mutex::new(None),
            foreground_format: Mutex::new(foreground::OutputFormat::default()),
            config: Mutex::new(config::Config::default()),
            skills_config: Mutex::new(skills::SkillsConfig::default()),
            skills_write_lock: Mutex::new(()),
            history: Mutex::new(history::HistoryStore::default()),
            http_client: reqwest::Client::new(),
            is_capturing: AtomicBool::new(false),
            is_pasting: AtomicBool::new(false),
            is_rewriting: AtomicBool::new(false),
            auth_session: Mutex::new(None),
            subscription: Mutex::new(auth::SubscriptionCache::default()),
        })
        .setup(|app| {
            // ── Load config, skills, history ──────────────────────────────────
            let config_path = app.path().app_config_dir()?.join("config.toml");
            // Absence of config.toml means this is the very first launch after
            // install, since `save` (triggered by any settings change) always
            // writes it. Used below to greet the user with the Settings window
            // so they know reWrite is running.
            let is_first_run = !config_path.exists();
            let mut loaded_config = config::load(&config_path);
            if is_first_run {
                // Write the file now so the Settings-on-launch greeting only
                // ever fires once, even if the user closes Settings without
                // changing anything.
                let _ = config::save(&loaded_config, &config_path);
            }
            let hotkey = loaded_config.hotkey.clone();
            let super_hotkey = loaded_config.super_hotkey.clone();
            let default_skill_id = loaded_config.default_skill_id.clone();
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            let bubble_enabled = loaded_config.bubble_enabled;
            let skills_path = app.path().app_config_dir()?.join("skills.json");
            let skills_has_synced_default = skills::file_has_default_skill_id(&skills_path);
            let mut loaded_skills = skills::load(&skills_path);
            if skills_has_synced_default {
                // Once the synced field exists, it is authoritative over the
                // compatibility mirror in config.toml (which may lag after a
                // partial write/crash). Repair only the mirror.
                if loaded_config.default_skill_id != loaded_skills.default_skill_id {
                    loaded_config.default_skill_id = loaded_skills.default_skill_id.clone();
                    let _ = config::save(&loaded_config, &config_path);
                }
            } else {
                // Older skills.json files predate this field. config.toml is
                // the one-time migration source; no skills file write here,
                // so its logical edit time is not falsely advanced pre-sync.
                loaded_skills.default_skill_id = default_skill_id;
            }
            *app.state::<AppState>().config.lock().unwrap() = loaded_config;
            *app.state::<AppState>().skills_config.lock().unwrap() = loaded_skills;

            let history_path = app.path().app_config_dir()?.join("history.json");
            let loaded_history = history::load(&history_path);
            *app.state::<AppState>().history.lock().unwrap() = loaded_history;

            // ── Subscription: seed last-known-good cache before any sync ──────
            // A slow/failed network sync must not silently present a paying
            // user as "Free" — load whatever the last successful sync wrote
            // before the (async, possibly failing) network sync even starts.
            let subscription_path = app.path().app_config_dir()?.join("subscription.json");
            if let Some(sub) = auth::load_subscription(&subscription_path) {
                *app.state::<AppState>().subscription.lock().unwrap() = sub;
            }

            // ── Auth: load session, refresh + sync in background ──────────────
            let auth_path = app.path().app_config_dir()?.join("auth.json");
            let maybe_session = auth::load_session(&auth_path);

            if let Some(ref s) = maybe_session {
                *app.state::<AppState>().auth_session.lock().unwrap() = Some(s.clone());
            }

            if let Some(session) = maybe_session {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let Some(state) = app_handle.try_state::<AppState>() else {
                        return;
                    };
                    let client = state.http_client.clone();
                    let auth_path = match app_handle.path().app_config_dir() {
                        Ok(d) => d.join("auth.json"),
                        Err(_) => return,
                    };

                    let session = if auth::is_expired(&session) {
                        match auth::refresh_session(&client, session).await {
                            Ok(refreshed) => {
                                let _ = auth::save_session(&refreshed, &auth_path);
                                *state.auth_session.lock().unwrap() = Some(refreshed.clone());
                                refreshed
                            }
                            Err(_) => {
                                auth::clear_session(&auth_path);
                                *state.auth_session.lock().unwrap() = None;
                                return;
                            }
                        }
                    } else {
                        session
                    };

                    let sync_app = app_handle.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(error) = sync::sync_all(&sync_app).await {
                            trace(&format!("cloud sync at startup failed: {error}"));
                        }
                    });

                    if let Ok(sub) = auth::sync_subscription(&client, &session.access_token).await {
                        persist_subscription(&app_handle, &sub);
                        *state.subscription.lock().unwrap() = sub;
                        // The frontend's initial read (on mount) can easily
                        // race this background sync; nudge it to re-read so a
                        // slow startup sync doesn't leave the UI stuck on
                        // whatever was cached (or default) at first paint.
                        let _ = app_handle.emit("subscription:updated", ());
                    }
                });
            }

            // ── 24h subscription refresh timer ────────────────────────────────
            {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut interval =
                        tokio::time::interval(tokio::time::Duration::from_secs(24 * 3600));
                    interval.tick().await; // skip the immediate tick
                    loop {
                        interval.tick().await;
                        if app_handle.try_state::<AppState>().is_none() {
                            break;
                        }
                        if let Some(token) = ensure_valid_token(&app_handle).await {
                            let Some(state) = app_handle.try_state::<AppState>() else {
                                break;
                            };
                            if let Ok(sub) =
                                auth::sync_subscription(&state.http_client, &token).await
                            {
                                persist_subscription(&app_handle, &sub);
                                *state.subscription.lock().unwrap() = sub;
                                let _ = app_handle.emit("subscription:updated", ());
                            }
                        }
                    }
                });
            }

            // ── Background update check ───────────────────────────────────────
            #[cfg(not(debug_assertions))]
            {
                use tauri_plugin_updater::UpdaterExt;
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let Ok(updater) = app_handle.updater() else {
                        return;
                    };
                    let Ok(Some(update)) = updater.check().await else {
                        return;
                    };
                    if update.download_and_install(|_, _| {}, || {}).await.is_ok() {
                        app_handle.request_restart();
                    }
                });
            }

            // ── Deep-link handler ─────────────────────────────────────────────
            {
                use tauri_plugin_deep_link::DeepLinkExt;

                // Register the scheme in the Windows registry during development.
                // Runtime registration is unsupported on macOS; packaged builds
                // get their scheme registration from tauri.conf.json instead.
                #[cfg(all(debug_assertions, target_os = "windows"))]
                app.deep_link().register_all()?;

                let app_handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        handle_deep_link(&app_handle, url.as_str());
                    }
                });

                // Handle URLs that launched the app (e.g. clicked link while app was closed)
                if let Ok(Some(urls)) = app.deep_link().get_current() {
                    for url in urls {
                        handle_deep_link(app.handle(), url.as_str());
                    }
                }
            }

            // ── Hotkeys ───────────────────────────────────────────────────────
            let hotkey_ok = app.global_shortcut().register(hotkey.as_str()).is_ok();
            if !hotkey_ok {
                eprintln!("Failed to register hotkey '{hotkey}'");
            }

            if super_hotkey != hotkey {
                if app
                    .global_shortcut()
                    .register(super_hotkey.as_str())
                    .is_err()
                {
                    eprintln!("Failed to register super hotkey '{super_hotkey}'");
                }
            }

            // ── Selection watcher ────────────────────────────────────────────
            // Background service for v1.1.0's selection bubble (see
            // selection_watcher.rs) — on by default, but user-toggleable via
            // Settings (Sprint 4's `bubble_enabled` config flag) for RTS-style
            // click-drag games / users who find the popup intrusive.
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            if bubble_enabled {
                selection_watcher::start(app.handle());
            }

            // ── Pre-warm overlay ──────────────────────────────────────────────
            if let Ok(overlay) = tauri::WebviewWindowBuilder::new(
                app.handle(),
                "overlay",
                tauri::WebviewUrl::App("".into()),
            )
            .title("")
            .decorations(false)
            .always_on_top(true)
            .transparent(true)
            .skip_taskbar(true)
            .inner_size(480.0, 430.0)
            .center()
            .focused(false)
            .visible(false)
            .build()
            {
                let overlay_ref = overlay.clone();
                overlay.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = overlay_ref.hide();
                        #[cfg(any(target_os = "windows", target_os = "macos"))]
                        esc_hook::stop();
                    }
                });

                // ── Warm the overlay's webview ────────────────────────────
                // A window built hidden keeps its WebView2 content cold: the
                // engine defers loading the page until the window is first
                // shown. That made the very first overlay show race the cold
                // start — the native window was up (so Alt+F4 closed it) but
                // React and the Tauri IPC weren't live yet, so Esc and the X
                // button did nothing until a later show warmed it. Park the
                // window far off-screen and show it to force the webview to
                // load and mount React; React emits "overlay:ready", at which
                // point we hide it again. The next real show re-centers (see
                // show_overlay), so the off-screen parking stays invisible.
                let _ = overlay.set_position(PhysicalPosition::new(-32000, -32000));
                let warm_hide = overlay.clone();
                overlay.once("overlay:ready", move |_| {
                    if OVERLAY_OPENED.load(Ordering::SeqCst) {
                        trace("warm: overlay:ready but overlay already opened -> skip hide");
                        return;
                    }
                    trace("warm: overlay:ready received -> hide");
                    let _ = warm_hide.hide();
                    trace("warm: overlay:ready hide done");
                });
                // Safety net: if "overlay:ready" never arrives, don't leave the
                // window parked-and-shown off-screen forever.
                let warm_fallback = overlay.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    if OVERLAY_OPENED.load(Ordering::SeqCst) {
                        trace("warm: 5s fallback but overlay already opened -> skip hide");
                        return;
                    }
                    trace("warm: 5s fallback -> hide");
                    let _ = warm_fallback.hide();
                    trace("warm: 5s fallback hide done");
                });
                trace("warm: overlay.show() (off-screen) start");
                let _ = overlay.show();
                trace("warm: overlay.show() (off-screen) done");
            }

            // ── Pre-warm processing indicator ─────────────────────────────────
            if let Ok(proc_win) = tauri::WebviewWindowBuilder::new(
                app.handle(),
                "processing",
                tauri::WebviewUrl::App("".into()),
            )
            .title("")
            .decorations(false)
            .shadow(false)
            .always_on_top(true)
            .transparent(true)
            .skip_taskbar(true)
            .inner_size(240.0, 240.0)
            .center()
            .focused(false)
            .visible(false)
            .build()
            {
                let proc_ref = proc_win.clone();
                proc_win.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = proc_ref.hide();
                    }
                });
            }

            // ── Pre-warm selection bubble ──────────────────────────────────────
            // The visible ring is only BUBBLE_VISIBLE_SIZE (Bubble.tsx), but the
            // window itself is BUBBLE_HIT_PADDING larger on each side — a live
            // trace session showed clicks aimed at a window sized to match the
            // visible dot routinely missing it outright (landing on the source
            // app instead, which silently cleared the selection before
            // `bubble_clicked` ever fired). The extra window space is invisible
            // (transparent) but still clickable, giving a much more forgiving
            // hit target without changing how big the dot looks.
            if let Ok(bubble) = tauri::WebviewWindowBuilder::new(
                app.handle(),
                "bubble",
                tauri::WebviewUrl::App("".into()),
            )
            .title("")
            .decorations(false)
            .shadow(false)
            .always_on_top(true)
            .transparent(true)
            .skip_taskbar(true)
            .inner_size(
                BUBBLE_VISIBLE_SIZE + BUBBLE_HIT_PADDING * 2.0,
                BUBBLE_VISIBLE_SIZE + BUBBLE_HIT_PADDING * 2.0,
            )
            .focused(false)
            // Without this, macOS treats a click on this (deliberately
            // unfocused, so it doesn't steal focus from the source app)
            // window as pure activation and never delivers it to the
            // webview's JS click handler — the bubble visibly appears but
            // clicking it does nothing. This is Tauri/wry's wrapper around
            // `NSWindow.acceptsFirstMouse`; harmless on non-macOS targets.
            .accept_first_mouse(true)
            .visible(false)
            .build()
            {
                let bubble_ref = bubble.clone();
                bubble.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = bubble_ref.hide();
                    }
                });
            }

            // ── Pre-warm bubble menu ───────────────────────────────────────────
            // Build this WebView at startup. Creating a fresh WebView from the
            // bubble click path can stall on Windows; keeping this prebuilt
            // one alive and resetting it by event gives us fresh state without
            // constructing or reloading a window during the interaction.
            if let Ok(bubble_menu) = tauri::WebviewWindowBuilder::new(
                app.handle(),
                "bubble_menu",
                tauri::WebviewUrl::App("".into()),
            )
            .title("")
            .decorations(false)
            .shadow(false)
            .always_on_top(true)
            .transparent(true)
            .skip_taskbar(true)
            .inner_size(BUBBLE_MENU_WIDTH, BUBBLE_MENU_HEIGHT)
            .position(BUBBLE_MENU_PARKED_X, BUBBLE_MENU_PARKED_Y)
            .focused(false)
            // Same fix as the bubble window above — without this, clicking a
            // skill in this unfocused menu would be swallowed as pure window
            // activation on macOS instead of reaching the click handler.
            .accept_first_mouse(true)
            .visible(true)
            .build()
            {
                let app_handle = app.handle().clone();
                bubble_menu.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        hide_bubble_menu(&app_handle);
                    }
                });
            }

            // ── Selection watcher listeners ────────────────────────────────────
            // Always-on background service (see selection_watcher.rs) emits these
            // two events from its own worker thread; react by showing/hiding the
            // bubble. Registered once, for the app's lifetime.
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            {
                let app_handle = app.handle().clone();
                app.listen("selection:detected", move |event| {
                    if let Ok(payload) =
                        serde_json::from_str::<selection_watcher::AnchorPayload>(event.payload())
                    {
                        show_bubble(&app_handle, payload.x, payload.y);
                    }
                });

                let app_handle = app.handle().clone();
                app.listen("selection:cleared", move |_event| {
                    hide_bubble(&app_handle);
                });
            }

            // ── Pre-warm settings ─────────────────────────────────────────────
            // Build the (large, webview-heavy) Settings window once, hidden, so
            // that opening it later is a cheap show()/set_focus(). Building it on
            // demand from the overlay's `open_settings` command deadlocked the
            // main event loop on Windows, leaving both the overlay stuck on
            // screen and the Settings webview blank. Pre-warming sidesteps that
            // entirely and makes the window paint instantly when revealed.
            if let Ok(settings) = tauri::WebviewWindowBuilder::new(
                app.handle(),
                "settings",
                tauri::WebviewUrl::App("".into()),
            )
            .title("reWrite - Settings")
            .decorations(true)
            .always_on_top(false)
            .inner_size(1260.0, 870.0)
            .min_inner_size(900.0, 600.0)
            .center()
            .resizable(true)
            .visible(false)
            .build()
            {
                let settings_ref = settings.clone();
                settings.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        // Keep the window warm: hide instead of destroying it so
                        // it can be reopened instantly and never needs rebuilding.
                        api.prevent_close();
                        let _ = settings_ref.hide();
                    }
                });
            }

            // ── Tray ──────────────────────────────────────────────────────────
            let settings_item = MenuItemBuilder::new("Settings").id("settings").build(app)?;
            let quit_item = MenuItemBuilder::new("Quit reWrite").id("quit").build(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[&settings_item, &quit_item])
                .build()?;

            let tooltip = if hotkey_ok {
                format!("reWrite  ·  {hotkey}")
            } else {
                format!("reWrite  ·  ⚠ hotkey '{hotkey}' unavailable")
            };

            TrayIconBuilder::new()
                .icon(tauri::include_image!("icons/rewrite_logo_taskbar.png"))
                .menu(&menu)
                .tooltip(&tooltip)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "settings" => show_settings(app),
                    "quit" => {
                        // Chrome counts AXEnhancedUserInterface enable/disable
                        // requests. Balance any activation owned by the macOS
                        // passive watcher before normal tray exit rather than
                        // leaving Chrome in complete AX mode until Chrome quits.
                        #[cfg(target_os = "macos")]
                        selection_watcher::stop();
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            // ── First run: open Settings so the user knows reWrite is running ──
            if is_first_run {
                show_settings(app.handle());
            }

            // ── macOS: Accessibility permission tutorial (roadmap-mac.md Phase 2) ──
            // If Accessibility isn't granted, surface Settings immediately on
            // launch so the user lands on the tutorial (`AccessibilityView.tsx`,
            // shown by `Settings.tsx` when `check_accessibility_permission`
            // comes back false) instead of discovering it later via a silently
            // failing hotkey. `is_first_run` already opened Settings above (and
            // the frontend will detect the missing permission on mount and
            // switch straight to the tutorial view), so this only needs to
            // handle the non-first-run case. Non-macOS builds don't have this
            // permission concept, so behavior there is unchanged.
            #[cfg(target_os = "macos")]
            if !is_first_run && !clipboard::accessibility_trusted(false) {
                show_settings(app.handle());
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_captured_text,
            commands::get_capture_error,
            commands::rewrite_with_skill,
            commands::paste_text,
            commands::get_config,
            commands::save_config,
            commands::open_settings,
            commands::close_overlay,
            commands::update_hotkey,
            commands::update_super_hotkey,
            commands::set_default_skill,
            commands::get_skills_config,
            commands::save_skills_config,
            commands::create_skill,
            commands::delete_skill,
            commands::reorder_skills,
            commands::toggle_builtin_skill,
            commands::get_history,
            commands::get_auth_state,
            commands::send_magic_link,
            commands::open_google_login,
            commands::logout,
            commands::open_checkout,
            commands::open_billing_portal,
            commands::refresh_subscription,
            commands::set_display_name,
            commands::bubble_clicked,
            commands::close_bubble_menu,
            commands::debug_trace,
            commands::check_accessibility_permission,
            commands::request_accessibility_permission,
            commands::open_accessibility_settings,
            commands::is_macos,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
