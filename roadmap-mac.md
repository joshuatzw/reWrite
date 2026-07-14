# reWrite — Mac Roadmap

This roadmap tracks the work needed to bring the existing reWrite experience to macOS. The basic rewrite flow should remain the same: select text anywhere, invoke reWrite, choose or apply a skill, and paste the result back into the source app without forcing a context switch.

## Product goals

- Preserve the current core loop: capture selected text, rewrite it, and paste the result back into the original app.
- Make macOS permissions understandable before they become a failure state.
- Add a lightweight floating bubble when text is highlighted, so users can rewrite without remembering a hotkey.
- Keep the hotkey path available as the reliable fallback for apps where passive selection detection is limited.

## Decisions to make

| Area | Options | Default |
|---|---|---|
| Permission onboarding | First-run tutorial / Settings-only guide / both | Both |
| Selection trigger | Accessibility API polling / mouse-up probe / hybrid | Hybrid |
| Bubble behavior | Always on by default / opt-in | On by default with Settings toggle |
| App compatibility | Best effort across all apps / explicit supported-app list | Best effort with known limitations |
| Paste behavior | Simulated Cmd+V / direct app integration where possible | Simulated Cmd+V |

## Phase 1 — macOS app foundation

- [ ] Confirm Tauri macOS build configuration, bundle identifier, app icon, and signing requirements.
- [ ] Map Windows-only modules to macOS equivalents:
  - [x] `clipboard.rs`: Cmd+C / Cmd+V capture and paste behavior.
    Code path exists and passes build/test via `capture_selection`,
    `paste_and_restore`, and `paste_html_and_restore` using the macOS Meta key
    plus Accessibility gating. Not real-device verified across target apps.
  - [x] `esc_hook.rs`: global escape or dismissal handling. Implemented
    2026-07-11 via a macOS `CGEventTap` (see `project.md` Recent Updates for
    the full design writeup). Code-complete and passes `cargo
    check`/`build`/`test`; NOT verified on a real device — no packaged app
    run, no real Escape keypress observed dismissing a real overlay. See
    `project.md` Known Gaps for exactly how a human should verify it.
  - [x] `foreground.rs`: active app/window detection.
    Code path exists via main-threaded `NSWorkspace` lookup and bundle-id
    classification. Not runtime-verified; browsers remain plain-text on macOS
    because tab/window titles are not read.
  - [x] `selection_watcher.rs`: passive selection detection.
    Implemented 2026-07-11 as a first-pass macOS `CGEventTap` + Accessibility
    watcher that probes `AXSelectedText`/`AXBoundsForRange` and emits the
    existing bubble events. Code-complete and passes build/test; NOT verified
    on a real device. Multi-monitor clamping and app-compat testing remain
    open.
- [ ] Verify global shortcut registration for the existing hotkeys.
- [ ] Confirm app windows behave correctly on macOS:
  - Overlay appears near the active display or selection target.
  - Processing window does not steal focus unnecessarily.
  - Settings window opens normally from tray/menu bar.
- [ ] Add a macOS menu bar/tray equivalent with Settings and Quit.

## Phase 2 — Accessibility permission tutorial

macOS will require Accessibility permission for system-wide capture, paste, hotkeys, and passive selection behavior. This needs to feel guided, not scary.

- [x] Add first-run permission detection for Accessibility.
- [x] Add a tutorial screen explaining why reWrite needs Accessibility:
  - Read the currently selected text.
  - Show the rewrite picker near the selection.
  - Paste rewritten text back into the original app.
- [x] Provide a clear button to open the correct macOS Settings page.
- [x] Show a short step-by-step checklist:
  - Open System Settings.
  - Go to Privacy & Security → Accessibility.
  - Enable reWrite.
  - Return to reWrite.
- [x] Detect when permission has been granted and continue automatically.
- [x] Add a recovery path in Settings for users who skipped onboarding or revoked permission later.
- [x] Add clear degraded-state messaging when Accessibility is missing:
  - Hotkey capture may fail.
  - Floating bubble will be disabled.
  - Settings and account features still work.

  Implemented 2026-07-11: see `project.md` Recent Updates for the full
  breakdown (`AccessibilityView.tsx`, `check_accessibility_permission` /
  `request_accessibility_permission` / `open_accessibility_settings` Tauri
  commands, sidebar recovery entry, Home dashboard banner). Not verified on a
  real device — no packaged app run, no click-through of the native
  permission dialog or the System Settings deep link. Do not check off the
  Manual test matrix or Release checklist rows below from this alone.

## Phase 3 — Hotkey rewrite flow on Mac

- [x] Implement selected-text capture with Cmd+C simulation and clipboard snapshot/restore.
- [x] Implement paste-back with Cmd+V simulation and configurable paste delay.
- [x] Preserve the existing rewrite pipeline:
  - Overlay hotkey opens the skill picker.
  - Super hotkey applies the default skill silently.
  - Processing window appears while rewrite is running.
  - Usage-limit and billing errors surface with the same upgrade path.
- [ ] Test capture and paste in common Mac apps:
  - Notes
  - Mail
  - Safari
  - Chrome
  - Slack
  - Notion
  - Google Docs
  - Microsoft Word
  - VS Code
- [ ] Document app-specific limitations where selection capture or paste behavior differs.

## Phase 4 — Floating bubble on text highlight

This is the Mac version of the passive selection bubble: when the user highlights text, reWrite shows a small floating bubble near the selected text. Clicking it opens a compact skill menu.

- [x] Build a macOS selection watcher that detects likely text-selection completion.
- [x] Use the macOS Accessibility API to confirm a real text selection before showing the bubble.
- [x] Capture:
  - Selected text.
  - Selection bounding rectangle where available.
  - Source app/window identity for paste-back focus.
- [x] Show a small always-on-top bubble near the selection endpoint.
- [x] Clamp bubble position to the visible display area, including multi-monitor setups.
- [ ] Hide the bubble when:
  - [x] Selection is cleared.
  - [x] User types over or deletes the selection.
  - [ ] User switches apps.
  - [x] User presses Escape.
  - [x] Bubble menu opens.
- [x] Add a compact bubble menu with skill titles only.
- [x] Route bubble-menu actions through the same rewrite and paste pipeline as the overlay.
- [x] Add a Settings toggle for "Selection bubble".
- [x] Keep the hotkey flow independent so users can disable the bubble without losing core functionality.

  First-pass implementation added 2026-07-11. This is code-complete only:
  `cargo check`, `cargo build --lib --bins`, `cargo test --lib --bins`,
  `npx tsc --noEmit`, and `npm run build` pass, but the bubble has not been
  observed in a real macOS app. Do not check off the Manual test matrix or
  Release checklist bubble rows until real-device testing confirms detection,
  position, click-through, rewrite, paste-back, permission revoke/regrant, and
  multi-monitor behavior.

  Follow-up fix added 2026-07-11 after a real symptom report ("highlight text,
  no bubble"): if the app launched before Accessibility was granted, the
  watcher skipped startup and previously never restarted after the tutorial's
  permission poll turned green. The macOS permission commands now start the
  watcher when Accessibility is granted and the Selection bubble setting is on,
  and stop it when permission is missing/revoked. Still requires real-device
  verification.

  Second follow-up pass added 2026-07-11: filled three gaps found against the
  first-pass implementation. (1) `probe_selection` now has the
  Electron/web-app fallback described in this phase's plan —
  `AXUIElementCopyElementAtPosition` on the last mouse-up point when the
  focused element has no usable selection — matching the Windows backend's
  `ElementFromPoint` fallback. (2) Multi-monitor clamping is now real:
  `clamp_rect_to_monitor`'s macOS branch uses `CGGetDisplaysWithPoint`/
  `CGDisplayBounds` to find and clamp against the containing display's bounds
  (full display bounds, not a menu-bar/dock-excluded work area — see
  `src-tauri/src/lib.rs`'s `mod mac_display` doc comment for why). (3) Clicking
  outside the bubble menu now closes it on macOS too — the mouse tap now also
  taps `kCGEventLeftMouseDown`, computes drag distance the same way the
  Windows hook does, and calls the previously Windows-only
  `maybe_close_bubble_menu_on_outside_click` (now cross-platform; its body was
  already generic Tauri window calls). Coordinate-space and AX
  thread-affinity questions were researched (not assumed) this pass — see
  `project.md`'s Known Gaps for the findings and sourcing. Added unit tests
  for the new pure logic (`clamp_point_to_bounds`, `selection_anchor_from_rect`,
  `is_selection_significant`). Still requires real-device verification — see
  `project.md`.


  ##Phase 4B:
  1. macOS: The bubble menu has excess space below the available options, sizing should be dynamic depending on the number of skills. Maximum of 4 skills in container no wrap text, scrollable
2. Version number inconsistency
    1. Sidebar says v1.0.6, but in settings it is v1.0.7
3. Skill needs to be saved on the cloud so it can be update between devices
    1. 
4. Accessibility Menu 
- This menu is appearing on windows as well —> This should only appear if user opens it on a mac.
- "Granted! reWrite is ready to go.” is green by default —> this needs to be truly dependent on whether the system has granted rights to it.

## Phase 4C — Fix: bubble never appears in Chrome (and other Chromium-based apps)

Real symptom report 2026-07-12: "rewrite bubble is not coming up on Chrome browser." Investigated live against a `npm run tauri dev` build with trace logging, not just by reading code.

- [x] Reproduce and isolate the failure via trace logs.
  - Control test in Notes: `probe_selection` returned `Some(...)` on the first try, `selection:detected` fired, the bubble showed, clicking it opened the bubble menu, and `selection:cleared` fired correctly on deselect — confirms the detection → event → frontend-render pipeline itself is fine.
  - Test in Chrome: `probe_selection -> None` on every attempt across a multi-minute session, so `selection:detected` never fires and no bubble ever shows.
  - Test in Notion (Electron/Chromium-based desktop app): same `None`-on-every-attempt symptom as Chrome.
- [x] Identify root cause: Chromium-based renderers (Chrome, and Electron apps like Notion, which embed Chromium) only populate the accessibility tree that `AXSelectedText`/`AXSelectedTextRange`/`AXBoundsForRange` read from once they detect an actively-watching assistive-technology client (normally VoiceOver). This module's passive, read-only AX polling never triggers that activation, so both `probe_selection` code paths (focused element, and the Phase 4 Electron hit-test fallback) find a real AX element at the cursor/focus but it never carries selection data — indefinitely, not intermittently.
- [x] Implement fix in `selection_watcher.rs` (mac backend only): when both selection lookups in `probe_selection` come back empty but did find a real element, read that element's pid (`AXUIElementGetPid`) and call `maybe_activate_manual_accessibility(pid)`, which sets Chromium's documented `AXManualAccessibility` attribute to `true` on the app's own `AXUIElementRef` (`AXUIElementCreateApplication(pid)`, not the system-wide element used elsewhere in this file). This forces the target process into full accessibility mode, the same state VoiceOver would put it in. Attempted once per pid (`AX_ACTIVATED_PIDS`, never evicted — re-asking a reused pid is a harmless no-op); non-Chromium apps simply return `kAXErrorAttributeUnsupported` for the attribute set, which is discarded, so no bundle-id/browser allowlist is needed — the same code path runs for every app.
- [x] `cargo check --lib --bins`, `cargo test --lib --bins` (23 tests, unchanged) pass.
- [ ] **Real-device re-verification of the fix is still needed** — see caveat below before checking this box.

  **Known limitation of the fix, by design:** activating the tree doesn't populate it instantly. The very first selection in a given Chrome/Notion window right after this fires may still show no bubble; the *next* selection in that same process should work, since the activation (and the tree) persists for the process's whole lifetime — `maybe_activate_manual_accessibility` only ever asks once per pid. A human should: launch/rebuild the app, highlight text in Chrome (first attempt may still miss — check the trace log for `maybe_activate_manual_accessibility(mac): pid=<chrome_pid> ... err=0`, meaning the attribute was accepted), then highlight a second time in the same Chrome window and confirm the bubble now appears. Repeat for Notion. `err=0` means Chromium accepted the attribute; a non-zero `err` there (as opposed to the expected `-25205`/attribute-unsupported seen on plain native apps) would mean the activation itself is failing and needs further investigation. Telegram's macOS client is not Chromium-based, so if it was also failing before this fix, that is a separate, still-open issue — re-test it independently once Chrome/Notion are confirmed fixed.

### 2026-07-14 live re-verification — the `AXManualAccessibility` fix is NOT sufficient for Chrome *web* content

Re-ran the live `npm run tauri dev` + trace investigation against real Chrome selections in a dedicated test page (labeled `<textarea>`, `<input>`, `contenteditable`, and a read-only paragraph). Accessibility was granted and the watcher was confirmed running. **Chrome web-page editable fields still never trigger the bubble.** The earlier Phase 4C fix addressed the wrong layer. Trace-confirmed root causes, all three probe paths dead-ended:

1. **Point hit-test returns the container, not the field.** `AXUIElementCopyElementAtPosition` at the selection point inside a Chrome web view consistently returns the top-level `AXScrollArea` (web-content container), never the `<textarea>`/`<input>`/contenteditable descendant under the cursor. `AXScrollArea` is correctly not-editable, so the probe stops there.
2. **`AXManualAccessibility` is rejected on the reachable element.** Setting it on that `AXScrollArea` returns `-25205` (attribute-unsupported) — so on web content the activation the Phase 4C fix relies on *fails*, and the deeper tree never populates. (It only succeeds, `err=0`, on elements we can't reach for web selections.)
3. **Focused-element resolution is empty/unavailable for Chrome.** `system.AXFocusedUIElement` returns nothing for Chrome web fields, and `system.AXFocusedApplication` returns `-25212` (`kAXErrorNoValue`) when Chrome is frontmost. (Both work for native + Electron apps like VS Code — that is why mid-debug attempts using the focused-application element resolved to the *wrong* app when a native app happened to hold keyboard focus.)

Net: there is a real **bootstrapping problem** — every element reachable via point hit-test or focused-element lookup either isn't the editable field or rejects the activation that would expose the selection, so Chrome's web AX tree stays dark to passive polling.

- [x] Real-device re-verification done (2026-07-14): confirms Chrome web-editable selection still does **not** show the bubble. Reverted all diagnostic scaffolding.
- [x] Shipped the reviewed **editable-role gate** (`is_editable_role` + role-based branch in `is_element_editable`, mac backend) — correct, unit-tested improvement that classifies editable-vs-read-only for elements the probe *can* reach (native controls, WebKit, Chrome's native address bar). Does not, by itself, resolve the Chrome web-content case above.
- [ ] **Still open — Chrome/Chromium web-content selection detection.** Candidate approaches, in rough order of promise:
  - **App-element via frontmost main pid.** Get Chrome's *main* process pid (not the renderer pid the web nodes report — e.g. via `NSWorkspace.frontmostApplication`, which `lib.rs`'s `remember_paste_target_window` already uses), `AXUIElementCreateApplication(main_pid)`, assert `AXManualAccessibility`, then read `AXFocusedUIElement`. Uncertain: prior work found app-level `AXManualAccessibility` *set* returns `-25205`; needs empirical check of whether the read still resolves the field once activated.
  - **Delayed re-probe after activation.** Since the tree populates asynchronously, schedule a second probe ~150–300ms after a first activation on a given pid, instead of relying on the next user selection.
  - **Explicit gesture + clipboard capture for Chromium.** Fall back to a hotkey/gesture that copies via Cmd+C and reads the clipboard (reusing the existing `clipboard.rs` + overlay infra). Reliable across Chrome/Electron/native, but changes the UX from passive-on-highlight to highlight-then-trigger for these apps.

## Phase 5 — Mac polish and reliability

- [ ] Add compatibility tracing for failed capture, paste, and bubble detection cases.
- [ ] Add user-facing troubleshooting for Accessibility permission, clipboard permission prompts, and unsupported apps.
- [ ] Verify dark mode and light mode styling for:
  - Settings
  - Overlay
  - Processing window
  - Bubble
  - Bubble menu

  Implementation added 2026-07-11: a centralized `--rw-*` CSS custom-property
  color system (`src/theme.css`, imported from `src/index.css`) with light
  defaults and a `@media (prefers-color-scheme: dark)` override block. All
  five windows listed above had their inline `style={{...}}` hex-literal
  colors converted to reference these tokens — `Settings.tsx` and
  `settingsComponents.tsx` (Settings, including the Sidebar), `Overlay.tsx`,
  `Processing.tsx`, `Bubble.tsx`, and `BubbleMenu.tsx`. `Bubble.tsx`'s dot and
  `BubbleMenu.tsx`'s loading spinner deliberately keep their existing fixed
  palette (documented in both files) since they float over arbitrary,
  unpredictable app content and need to stay legible against anything, not
  follow the system theme; `Processing.tsx`'s glow shadows are the same case.

  Same-day critical-review follow-up: `AccessibilityView.tsx` (one of
  Settings' five internal views — it auto-opens on first run and on any
  Accessibility revoke, so it's not a rare corner) was converted too, closing
  the one gap the first pass had left; a Toggle on/off dark-mode contrast bug
  (both states read as the same dark-gray pill) was fixed with a new
  `--rw-toggle-off` token. Box-shadow elevation colors remain intentionally
  unconverted (documented future polish, not a correctness issue — see
  `project.md`'s Known Gaps).

  `npx tsc --noEmit` and `npm run build` both pass, and the compiled CSS was
  confirmed to contain the dark-mode media query and correct light/dark
  values for every token (27 total, no orphans). **Left unchecked
  deliberately** — this item says "verify," and nothing in this environment
  can render a browser or take a screenshot, so none of this has been
  visually confirmed. Implementation is now believed complete across all five
  windows; the only remaining blocker to checking this box is the visual
  pass itself. A human needs to set macOS Appearance to Dark, open every
  Settings view (Home, History, Skills, Settings, Accessibility — including
  landing on Accessibility with permission revoked), trigger the Overlay and
  Processing windows, highlight text to see the Bubble/Bubble menu, and
  toggle a Settings switch to confirm on/off are visibly distinct, checking
  each for legible text and visible borders against the dark surfaces. Only
  check this box after that's actually been done. See `project.md`'s Known
  Gaps for the full writeup, including the one intentionally-left literal (a
  `<select>` chevron baked into a data-URI SVG that can't reference a CSS
  variable).
- [ ] Verify behavior across display setups:
  - Built-in display only.
  - External monitor.
  - Mixed Retina and non-Retina scaling.
  - Selection near screen edges.
- [ ] Confirm no noticeable input lag during rapid clicking, dragging, or typing.
- [ ] Confirm the bubble does not appear during non-text drag actions where possible.

## Manual test matrix

| Scenario | Expected result |
|---|---|
| First launch without Accessibility permission | Tutorial appears and explains how to enable permission |
| Permission granted while app is open | App detects permission and continues without restart if possible |
| Highlight text in a supported app | Bubble appears near selection |
| Click bubble | Bubble hides and compact skill menu appears |
| Choose a skill from bubble menu | Rewritten text replaces original selection |
| Press overlay hotkey | Full skill picker opens and works |
| Press super hotkey | Default skill rewrites and pastes without picker |
| Delete highlighted text while bubble is visible | Bubble disappears |
| Switch apps while bubble is visible | Bubble disappears |
| Disable Selection bubble in Settings | Bubble watcher stops and hotkeys still work |
| Revoke Accessibility permission | Bubble disables and app shows recovery guidance |

## Open questions

- Which macOS versions should be officially supported?
- Should the first Mac release ship with hotkeys first, then bubble, or wait until both are ready?
- Do we want app-specific allow/deny rules if the Accessibility API behaves inconsistently in certain apps?
- Should the tutorial include a short visual walkthrough or stay text-only for the first version?

## Release checklist

- [ ] Accessibility tutorial complete.
- [ ] Hotkey rewrite flow verified in top target apps.
- [ ] Floating selection bubble verified in top target apps.
- [ ] Settings toggle for bubble works live.
- [ ] Permission recovery path works after revoking Accessibility.
- [ ] Signed and notarized macOS build produced.
- [ ] Known limitations documented.
