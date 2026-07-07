# Selection Bubble / Bubble Menu — Bug Diagnosis Log

This documents the debugging journey for the v1.1.0 passive selection bubble
feature (see `v1.1.0-selection-bubble.md` for the original feature build).
After the initial 5-sprint build, real cross-app testing surfaced a chain of
bugs — each fix exposed the next issue underneath. This is written up as a
diagnosis log (symptom → investigation → root cause → fix) rather than a
simple changelog, since the *process* of tracking each one down is as useful
as the fixes themselves if something regresses later.

All fixes are in the working tree, uncommitted, as of this writing.

## Issue 1 — Bubble invisible in VS Code, unresponsive in WhatsApp

**Symptom:** In VS Code, the bubble never appeared to be visible. In
WhatsApp, a tiny bubble appeared but clicking it did nothing.

**Investigation:** No live repro tooling yet at this stage — reasoned from
the code. The original bubble was a 10×10px near-black dot (`#16161a`),
which would blend into a dark editor theme. Separately, `show_bubble` /
`show_bubble_menu` never asserted z-order, so an always-on-top source app
(e.g. a chat app's floating call window) could render above the bubble.

**Root cause:** Two independent contributing factors, not one bug:
- Low contrast (dark dot, dark theme).
- No z-order re-assertion against competing always-on-top windows.
- (Flagged but unconfirmed at the time: VS Code's Monaco editor may not
  expose a UI Automation `TextPattern` selection at all unless
  `editor.accessibilitySupport` is `"on"` — it's `"auto"` by default, which
  only enables the full accessibility tree when a screen reader is
  detected. This was never definitively confirmed as relevant.)

**Fix:**
- `Bubble.tsx` redesigned: white circle with a slow-rotating conic-gradient
  ring (blue → green → yellow → red), sized up from 10×10 to eventually
  20×20 (see Issue 2 for the intermediate 16×16 step).
- `show_bubble` / `show_bubble_menu` (`lib.rs`) now toggle
  `set_always_on_top(false)` → `set_always_on_top(true)` immediately before
  showing, forcing the window to the front of the topmost band regardless
  of what else claims to be always-on-top.

## Issue 2 — Bubble still didn't open the menu (WhatsApp / general)

**Symptom:** After Issue 1's fix, the bubble was visible, but clicking it
still did nothing in some apps.

**Investigation:** Launched a tracked `cargo run` / `npm run tauri dev`
session with `crate::trace(...)` instrumentation added to `bubble_clicked`,
`show_bubble`, `show_bubble_menu`, `hide_bubble`, `run_probe_cycle`. Watched
the trace live while reproducing.

**Root cause (two separate bugs found via trace evidence):**
1. **Hit target too small.** The window was sized to exactly match the
   16×16 visible ring — genuinely hard to land a click on precisely,
   especially at the exact corner coordinate the selection anchor
   computed.
2. **The real bug:** clicking the bubble is itself a `WM_LBUTTONUP` that the
   global low-level mouse hook (used for selection detection) also sees.
   ~200ms later (the existing debounce), the probe cycle ran again — and
   most apps don't clear their internal text selection just because a
   different window took focus, so UIA still reported the *same* selection
   as present. That re-emitted `selection:detected`, which re-called
   `show_bubble(...)`, popping the tiny dot back up **on top of** the menu
   that had just opened.

**Fix:**
- Bubble window enlarged: visible ring stays a fixed size, but the actual
  window gets an invisible `BUBBLE_HIT_PADDING` margin on each side (10px),
  making the click target much larger without changing the visual size.
- `run_probe_cycle` (`selection_watcher.rs`) now checks
  `bubble_menu_is_open()` first and skips the entire probe cycle while the
  menu is open — a single-source-of-truth guard, regardless of what UIA
  would otherwise report.

## Issue 3 — Menu opens then instantly closes itself (no menu ever visible)

**Symptom:** Click the bubble, it disappears (confirming `bubble_clicked`
fires), but the menu never visibly appears — not even a flash.

**Root cause:** `BubbleMenu.tsx`'s dismiss-on-blur listener closed the
window synchronously on the very first `focused: false` payload. Its
comment claimed to "mirror `Overlay.tsx`'s pattern" — checked directly, and
that's false; `Overlay.tsx` has no close-on-blur logic at all. Showing the
menu (`set_position` → `set_always_on_top` toggle → `.show()` →
`.set_focus()` on a window that was hidden a moment before) can produce a
transient/out-of-order activation blip on Windows — a stray `false` arriving
right around the real `true` — which the blur handler treated as "user
clicked away" and closed the menu before it was ever rendered.

**Fix:** The blur handler no longer closes synchronously off the raw event.
On `focused: false` it waits 150ms, then re-checks
`getCurrentWindow().isFocused()` before actually closing — a transient blip
self-corrects within that window; a real blur still closes it, just
slightly delayed.

Also added at this point: `crate::trace(...)` in `bubble_clicked`,
`show_bubble_menu`, `close_bubble_menu` — the instrumentation that made the
rest of this investigation possible.

## Issue 4 — Menu *still* didn't appear (the actual root cause, finally found via live trace)

**Symptom:** Same as Issue 3, after the blur-debounce fix.

**Investigation:** This time, launched a tracked dev session and watched
`crate::trace` output live while reproducing. The trace showed:

```
show_bubble: at (...) show=true
frontend: Bubble handleClick fired, payload=null   <-- the smoking gun
```

The click *did* land on the bubble (confirmed by the new hit-padding from
Issue 2). But `Bubble.tsx`'s own `selection:detected` listener — which it
used to populate the payload passed to `bubble_clicked` — hadn't received
the event yet, ~300ms after Rust had already emitted it and shown the
window. A human click beat the frontend's own event listener.

**Root cause:** Event delivery from Rust into a webview that was hidden a
moment earlier is not instant — there's a real, observable delay (at least
hundreds of ms) before the frontend's JS processes a queued Tauri event.

**Fix:** Stopped routing the click through a frontend-owned event listener
entirely. `selection_watcher.rs` now keeps its own `LAST_ANCHOR` (the same
data it already builds to emit `selection:detected`), exposed via
`last_anchor()`. `bubble_clicked` (Rust command) reads directly from that
instead of trusting arguments the frontend would have had to capture from
an event. `Bubble.tsx`'s click handler is now a one-liner:
`invoke("bubble_clicked")` — no payload, no race.

## Issue 5 — Stuck error state: once one rewrite errors, every subsequent bubble click just re-shows the same error

**Symptom:** A rejected rewrite (e.g. text outside the app's intended scope)
shows an error in the menu. Every bubble click afterward — even for
completely different, freshly selected text, even via the hotkey/Overlay
flow — kept showing that same stale error instead of the skill list.

**Investigation, attempt 1:** `BubbleMenu.tsx` is a single pre-warmed window,
reused for every selection (never remounted), so React state persists
across uses. The reset-to-idle logic only ran on the window's `focused`
event — already known to be an unreliable signal (see Issue 3). Fix
attempted: had Rust emit a dedicated `bubble_menu:opened` event every time
`show_bubble_menu` runs, and had `BubbleMenu.tsx` reset its state on that
event instead of relying on focus.

**Result: didn't work.** Retested — error still stuck.

**Investigation, attempt 2 (live trace):** Added `debug_trace` (a temporary
command letting frontend code emit lines into the same Rust trace output)
plus a render-logging effect in `BubbleMenu.tsx`, then watched a live
reproduction. The trace showed a working first cycle (fast `debug_trace`
round-trips, sub-10ms) followed by a second cycle where **nothing** fired
during the ~1–2 seconds the menu was shown before being dismissed again —
not `visibilitychange`, not the custom event, not even the same
`debug_trace` invoke that had worked instantly moments earlier.

**Root cause identified:** Once the `bubble_menu` window had gone through a
genuine hide → show cycle, its webview appeared to stop processing anything
at all for some unpredictable period afterward — consistent with WebView2
suspending/throttling a control once it's no longer actually being
displayed.

**Fix attempted:** Stopped using `hide()`/`show()` for this window entirely.
Redesigned it to be permanently `visible(true)` and instead moved off-screen
to a parked sentinel coordinate (`BUBBLE_MENU_PARKED_X/Y = -32000, -32000`)
when "closed" — reasoning that if the OS considers the window genuinely
visible at all times, WebView2 should never have reason to suspend it.
`bubble_menu_is_open()` and the outside-click bounds check were updated to
test window *position* against the parked sentinel instead of
`is_visible()`. Also had to catch and fix a related landmine this exposed:
`paste_text` (shared with the Overlay/hotkey flow) was calling a real
`window.hide()` on whichever window invoked it, which — now that
`show_bubble_menu` no longer calls `.show()` — would have permanently broken
the bubble menu after the very first successful paste. Redirected that path
through `hide_bubble_menu` (the off-screen park) for the `bubble_menu`
window specifically.

**Result: still didn't work.** Retested with trace — same signature as
before: the window opened, and during the entire time it was visible before
being dismissed again, *none* of the reset triggers fired. This ruled out
"real HWND hide" as the sole cause — moving off-screen (still technically
`WS_VISIBLE`) produced the identical symptom, suggesting Chromium/WebView2's
occlusion detection considers a window entirely outside any monitor's
bounds equivalent to hidden for throttling purposes, regardless of the raw
visibility flag.

**Final fix — reset on close, not on reopen:** Rather than continuing to
chase a reliable "this is a fresh open" signal into a window whose JS
liveness can't be trusted at that moment, the reset was moved to the
*opposite* end of the lifecycle: `hide_bubble_menu` (the one function every
close path — outside-click, explicit dismiss, and successful paste — now
goes through) emits `bubble_menu:reset` **immediately before** parking the
window. At that moment the window's JS is still guaranteed to be live,
because it was just the active, focused thing the user was looking at.
`BubbleMenu.tsx` resets its status/error state unconditionally on that
signal — regardless of whether the interaction that's ending was a success
or an error. By the time the menu is shown again, there's nothing left to
reset.

The earlier reopen-time triggers (`visibilitychange`, `bubble_menu:opened`,
`focused`) were left in place as a harmless fallback, but `bubble_menu:reset`
is the mechanism this now actually depends on.

**Status: implemented, not yet independently re-verified live** as of this
writing — needs another manual pass (trigger the error, then select fresh
text and click the bubble again; also worth confirming via the hotkey/
Overlay flow, since that's what surfaced the "even hotkey rewrites showed
the stale error" variant of this report).

## Issue 6 — Close-time reset still failed because parking raced event delivery

**Symptom:** The close-time reset design from Issue 5 still did not clear the
stale error reliably.

**Root cause:** `hide_bubble_menu` emitted `bubble_menu:reset` and immediately
moved the menu to `(-32000, -32000)` in the same main-thread task. Tauri event
delivery into the webview is asynchronous; `emit(...)` queues the event, but it
does not mean React has processed it. That left the same race in a smaller
form: WebView2 could treat the parked/off-screen control as occluded before
the JS listener ran, so the reset event was lost.

**Fix:** Turned the close-time reset into a handshake:
- `hide_bubble_menu` now emits `bubble_menu:reset` with a close generation and
  waits to park the menu.
- `BubbleMenu.tsx` clears idle/error/loading state, marks in-flight work as
  cancelled, and calls the new `bubble_menu_reset_ack(generation)` command.
- Rust parks the menu only when the ack matches the current generation.
- A short fallback timer still parks the menu if the ack is lost, so the window
  cannot get stranded on-screen.

This also fixes a related late-write edge case: if the menu is closed while a
rewrite is still loading, the eventual API error is ignored instead of being
written into a parked menu and shown on the next open.

**Result: still didn't work.** A follow-up trace showed the reset event still
never reached the frontend — no `bubble_menu:reset` log and no
`bubble_menu_reset_ack`. The fallback parked the menu every time, proving the
webview was not processing even the visible-window reset handshake.

**Attempted fix: disposable window.** Stop reusing `bubble_menu`: destroy the
old window and build a fresh one at the selection anchor. This would have made
stale React state impossible by construction.

**Result: worse.** A trace showed `show_bubble_menu: on main thread`, then no
`built fresh` and no `build failed`. The app kept running, but the menu never
appeared. This matches another known project lesson: constructing WebViews on
demand from an interaction path can stall on Windows.

**Current fix:** Return to a startup-prewarmed `bubble_menu`, but force native
WebView reloads instead of frontend reset events. `hide_bubble_menu` parks the
window and calls `reload()`; `show_bubble_menu` calls `reload()`, moves it
on-screen, reasserts topmost, and focuses it. The native reload should discard
the stale React error state without requiring the old JS instance to process a
custom reset event. A short Rust-side probe suppression still prevents the
closing click from immediately re-detecting the same source selection, and
`show_bubble_menu` arms a short outside-click grace window so the bubble click's
own `WM_LBUTTONUP` cannot close the menu it just opened.

**Regression found:** After restoring the prewarmed parked window, the bubble
stopped appearing. The trace showed `maybe_close_bubble_menu_on_outside_click`
firing repeatedly while the menu was already parked, followed by
`hide_bubble_menu: parked and reloaded window` and then
`run_probe_cycle: skipped, bubble_menu open/recently closed`. In other words,
ordinary mouse-ups were "closing" an already-closed menu and constantly arming
the short probe-suppression window, so selection detection never got a chance
to emit `selection:detected`. The outside-click path now first checks whether
`bubble_menu` is at the parked coordinate and returns without suppressing
probing when it is.

## Issue 7 — Successful bubble rewrite appears to do nothing

**Symptom:** After selecting a skill from the bubble menu, the rewrite
completed and the menu closed, but the rewritten text did not appear in the
source app.

**Root cause:** The bubble flow no longer performs a fresh synthetic copy from
the source app; it relies on the UIA text captured during selection detection.
That part worked. The failure was later: `bubble_menu` is an always-on-top
webview that steals foreground. `paste_text` parked/reloaded the menu and then
sent Ctrl+V after the configured delay, but there was no explicit focus restore
to the app that originally owned the selection. The paste keystroke could land
on the off-screen/reloaded menu webview instead of the source app.

**Fix:** `selection_watcher` now records the foreground HWND alongside the last
UIA selection. When `paste_text` is invoked by `bubble_menu`, Rust parks/reloads
the menu, calls `focus_last_source_window()` (`SetForegroundWindow`) to restore
the original target app, then proceeds with the existing paste delay and
clipboard paste.

## Cross-cutting lesson

Across issues 3–6, the pattern was the same: **do not trust a signal sent
into a Tauri/WebView2 window that was recently hidden, off-screen, or
otherwise not the active foreground content.** Focus events, custom Tauri
events, and even basic `invoke()` calls have all been observed to arrive
late or not at all under those conditions in this app. The reliable pattern
that emerged is stronger than "reset at the right time": for tiny transient
windows like `bubble_menu`, prefer native window/webview lifecycle operations
or Rust-owned state over any design that requires a stale frontend instance to
wake up and repair itself.

## Outstanding temporary diagnostics still in the code

The following were added purely for this investigation and should be
removed once Issue 5's fix is confirmed solid:
- `debug_trace` command (`commands.rs`) and its registration in `lib.rs`.
- The `dbg(...)` helper and all its call sites in `BubbleMenu.tsx` (mount,
  focus/visibility logging, render logging, `handleSelect` error logging).
- The various `crate::trace(...)` calls added throughout
  `selection_watcher.rs` / `lib.rs` / `commands.rs` beyond what the codebase's
  existing "temporary diagnostics" convention (see `lib.rs`'s
  `TRACE_START`/`trace()` doc comment) would otherwise keep long-term — worth
  a pass to decide which are worth keeping permanently (e.g.
  `run_probe_cycle`'s probe-result trace is arguably useful going forward)
  versus which were purely for this bug hunt.
