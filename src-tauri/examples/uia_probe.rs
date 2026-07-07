//! SPRINT 0 FEASIBILITY SPIKE — throwaway, not wired into the app.
//!
//! Goal: prove (or disprove) that we can call Windows UI Automation (UIA) from
//! Rust to detect a non-empty text selection anywhere on the system and get its
//! on-screen bounding rectangle (the basis for a v1.1.0 "selection bubble"
//! feature, positioned the same way Grammarly/Office position their inline
//! icons). Also probes a fullscreen-exclusive-window heuristic so the future
//! watcher can skip games.
//!
//! Run from `src-tauri/`:
//!     cargo run --example uia_probe
//!
//! Then alt-tab into Notepad / a browser / Word and select some text — the
//! loop below polls every 500ms and prints what it sees. Ctrl+C in the
//! terminal to quit (no special signal handling here; this is a disposable
//! probe, not a service).
//!
//! Dependency note: this uses the `windows` crate (not `windows-sys`, which is
//! what the rest of `rewrite` uses for its small, function-only Win32 calls).
//! windows-sys 0.52's `Win32_UI_Accessibility` feature defines every UIA COM
//! interface (`IUIAutomation`, `IUIAutomationTextPattern`, ...) as a bare
//! `*mut c_void` type alias with no generated vtable or methods — it's meant
//! for callers who hand-roll their own vtable structs, which would make this
//! spike (and any real implementation) far more error-prone than it needs to
//! be. The `windows` crate generates real method bodies for the same
//! interfaces (`element.GetCurrentPatternAs(...)`, `range.GetText(...)`,
//! etc.) and COM plumbing (`CoCreateInstance`, `Interface`, `BSTR`, `Result`).
//! It's scoped to `[target...dev-dependencies]` in Cargo.toml since only this
//! example uses it for now, and windows 0.61 was already present transitively
//! in Cargo.lock (via another dependency), so this added no new major version
//! to the dependency tree.

use std::time::Duration;

use windows::core::Result as WinResult;
use windows::Win32::Foundation::RECT;
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
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};

fn main() -> WinResult<()> {
    // NOTE: COINIT_MULTITHREADED (MTA), not apartment-threaded. This process
    // has no window and never pumps a Win32 message loop, and STA COM objects
    // rely on message pumping to marshal cross-thread/cross-process calls —
    // without a pump, an STA GetFocusedElement() call into another process's
    // UIA provider can hang. MTA sidesteps that. If Sprint 1's watcher ends up
    // needing STA (e.g. to register UIA *event* handlers, which like to call
    // back on the apartment that registered them), it will need a real
    // message loop on whichever thread owns the IUIAutomation instance.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok()?;

    let result = run();

    unsafe { CoUninitialize() };
    result
}

fn run() -> WinResult<()> {
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)? };

    println!("uia_probe running (polling every 500ms). Ctrl+C to quit.");
    println!("Select text in another window (Notepad / browser / Word) to test.\n");

    loop {
        match probe_selection(&automation) {
            Ok(Some((text, rects))) => {
                println!("[selection] text={text:?}");
                println!("[selection] bounding rects={rects:?}");
            }
            Ok(None) => println!("[selection] none"),
            Err(e) => println!("[selection] error: {e:?}"),
        }

        match is_foreground_fullscreen_exclusive() {
            Ok(is_fullscreen) => println!("[fullscreen_exclusive] {is_fullscreen}"),
            Err(e) => println!("[fullscreen_exclusive] error: {e:?}"),
        }

        println!("---");
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Returns `Some((selected_text, bounding_rects))` if the focused UI element
/// supports TextPattern and currently has a non-empty selection, `None` if
/// there's no selection (or the focused element doesn't support text at all —
/// this is exactly the case that filters out RTS unit-boxing / file-manager
/// drag-drop, since neither implements TextPattern).
fn probe_selection(
    automation: &IUIAutomation,
) -> WinResult<Option<(String, Vec<(f64, f64, f64, f64)>)>> {
    // GetFocusedElement fails outright for some apps/states (e.g. nothing
    // focused, or the app's accessibility tree is unavailable) — treat any
    // error here as "no selection" rather than propagating, since a real
    // watcher can't crash a polling loop on every unsupported foreground app.
    let element = match unsafe { automation.GetFocusedElement() } {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    let text_pattern: IUIAutomationTextPattern =
        match unsafe { element.GetCurrentPatternAs(UIA_TextPatternId) } {
            Ok(p) => p,
            Err(_) => return Ok(None), // focused element doesn't support TextPattern
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
unsafe fn parse_rect_safearray(sa: *mut windows::Win32::System::Com::SAFEARRAY) -> WinResult<Vec<(f64, f64, f64, f64)>> {
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
/// rect — i.e. no border/title bar peeking out. This is intentionally crude;
/// it will also flag legitimately borderless-maximized windows (some media
/// players, some browsers in "fullscreen" F11 mode). That's an acceptable
/// false positive for Sprint 1's purposes: skipping the (already free) UIA
/// check in those cases costs nothing, since none of them are places a user
/// is likely to be selecting inline text anyway.
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
