# Separation Of Concerns Sprint Map

This sprint map turns the current architecture review into followable work. The goal is not a big-bang rewrite. The goal is to create clear ownership so each future change has an obvious place to go.

## North Star

Each file should have one primary reason to change:

- UI files change when presentation or local interaction changes.
- Frontend hooks/services change when data loading, IPC calls, or reusable workflows change.
- Tauri commands change when the frontend/native API contract changes.
- Rust domain modules change when app behavior changes.
- Supabase functions change when server-side auth, billing, usage, or model-provider behavior changes.

## Working Rules

- Start every task by reading `infrastructure.md`.
- Update `infrastructure.md` whenever a file is added, removed, renamed, or materially changed.
- Keep refactors behavior-preserving unless the sprint explicitly says otherwise.
- Prefer small vertical slices that compile after each step.
- Do not move code and redesign behavior in the same commit unless unavoidable.

## Sprint 0 - Map And Guardrails

Status: Ready to use.

Goal: Make the codebase navigable for future agents and keep the map from going stale.

Tasks:

- [x] Add `agent.md` with the required agent workflow.
- [x] Add `infrastructure.md` as the source-of-truth file map.
- [x] Add this sprint map.
- [x] Add a short reference from `README.md` to `agent.md` and `infrastructure.md`.
- [ ] During every later sprint, update `infrastructure.md` in the same change as code edits.

Acceptance:

- A new agent can read `infrastructure.md` and know where to inspect first.
- Any changed file has an accurate responsibility entry in `infrastructure.md`.

## Sprint 1 - Frontend IPC Boundary

Goal: Stop React views from knowing raw Tauri command strings.

Why: Direct `invoke(...)` calls are scattered across `Settings.tsx`, `Overlay.tsx`, `BubbleMenu.tsx`, and `Bubble.tsx`. This makes command names, payload shapes, and error handling hard to audit.

Proposed files:

- Add `src/ipc.ts`
- Optionally add `src/ipcTypes.ts` if payload/result types grow
- Update `infrastructure.md`

Tasks:

- [ ] Create typed wrappers for common commands: config, skills, history, auth, billing, rewrite, paste, window actions, and diagnostics.
- [ ] Replace direct invokes in `Overlay.tsx`.
- [ ] Replace direct invokes in `BubbleMenu.tsx`.
- [ ] Replace direct invokes in `Bubble.tsx`.
- [ ] Replace direct invokes in `Settings.tsx` once the wrappers are stable.
- [ ] Keep command names centralized in `src/ipc.ts`.

Acceptance:

- UI files call named functions such as `rewriteWithSkill`, `pasteText`, `getSkillsConfig`, `openCheckout`, and `saveConfig`.
- Raw `invoke(` usage is either absent from React pages or limited to the IPC wrapper.
- TypeScript build passes.

## Sprint 2 - Shared Rewrite Selection Flow

Goal: Give the overlay and bubble menu one shared rewrite workflow.

Why: `Overlay.tsx` and `BubbleMenu.tsx` both load skill config, build skill items, handle loading/error state, call rewrite, call paste, and special-case quota errors.

Proposed files:

- Add `src/hooks/useSkillItems.ts`
- Add `src/hooks/useRewriteSelection.ts`
- Add `src/errors.ts` or `src/rewriteErrors.ts` for quota/scope error classification
- Update `infrastructure.md`

Tasks:

- [ ] Extract skill loading/building into `useSkillItems`.
- [ ] Extract rewrite/paste orchestration into `useRewriteSelection`.
- [ ] Centralize limit/scope error detection.
- [ ] Keep overlay-specific concerns in `Overlay.tsx`: preview, keyboard navigation, layout, full error copy.
- [ ] Keep bubble-menu-specific concerns in `BubbleMenu.tsx`: compact menu sizing, reset events, spinner window sizing.

Acceptance:

- Overlay and bubble menu share the same rewrite/paste workflow code.
- Overlay-specific and bubble-specific window behavior remains local to those components.
- Quota error detection is defined once.
- Existing overlay and bubble menu behavior remains intact.

## Sprint 3 - Split Settings Into Screens And Hooks

Goal: Reduce `Settings.tsx` into a coordinator instead of a many-screen monolith.

Why: `Settings.tsx` currently owns login, home dashboard, history, skills management, billing, preferences, updater, data loading, and many command handlers.

Proposed files:

- Add `src/pages/settings/LoginView.tsx`
- Add `src/pages/settings/HomeView.tsx`
- Add `src/pages/settings/HistoryView.tsx`
- Add `src/pages/settings/SkillsView.tsx`
- Add `src/pages/settings/SkillsLockedView.tsx`
- Add `src/pages/settings/SettingsView.tsx`
- Add `src/pages/settings/SkillModal.tsx` if modal code is large enough to isolate
- Add `src/pages/settings/useSettingsData.ts`
- Add `src/pages/settings/useSkillsConfig.ts`
- Add `src/pages/settings/usePreferences.ts`
- Update `infrastructure.md`

Tasks:

- [ ] Move `LoginView` out first because it has a narrow auth concern.
- [ ] Move `HomeView` and keep dashboard calculations in `settingsHelpers.ts`.
- [ ] Move `HistoryView` and keep filtering/grouping either in the view or a small hook.
- [ ] Move `SkillsView` and its modal/actions into a settings skill module.
- [ ] Move `SettingsView` preferences, billing, logout, and updater into its own file.
- [ ] Leave root `Settings.tsx` responsible for auth gate, active tab, top-level data loading, and layout only.

Acceptance:

- `Settings.tsx` is small enough to understand as the settings-window shell.
- Each settings screen can be opened without reading unrelated screens.
- No behavior changes except import paths.
- TypeScript build passes.

## Sprint 4 - Thin The Tauri Command Layer

Goal: Make `commands.rs` an IPC adapter instead of the home of business workflows.

Why: Commands currently mix state locking, auth, config, prompt building, API calls, history logging, subscription cache updates, window behavior, and persistence.

Proposed files:

- Add `src-tauri/src/rewrite_flow.rs`
- Add `src-tauri/src/config_service.rs` only if config command logic keeps growing
- Add `src-tauri/src/skills_service.rs` only if skill command logic keeps growing
- Update `src-tauri/src/lib.rs` module exports
- Update `infrastructure.md`

Tasks:

- [ ] Move `rewrite_with_skill` orchestration into `rewrite_flow::rewrite_with_skill`.
- [ ] Move history logging helper out of `commands.rs` if it belongs with rewrite flow or history.
- [ ] Keep `commands::rewrite_with_skill` as a thin wrapper that converts errors to strings.
- [ ] Consider moving config update side effects, such as starting/stopping the selection watcher, behind a service function.
- [ ] Keep command signatures stable for the frontend.

Acceptance:

- `commands.rs` is easier to scan as a list of IPC commands.
- Rewrite orchestration can be tested or reasoned about without Tauri command macros.
- Existing frontend command contract still works.
- Rust check/build passes.

## Sprint 5 - Split Tauri Startup And Window Control

Goal: Break up `lib.rs` by concern.

Why: `lib.rs` currently contains global state, tracing, paste-target helpers, window helpers, bubble/menu positioning, app setup, auth sync timers, updates, deep links, hotkeys, prewarming, tray setup, and selection listeners.

Proposed files:

- Add `src-tauri/src/windows.rs`
- Add `src-tauri/src/hotkeys.rs`
- Add `src-tauri/src/startup.rs`
- Add `src-tauri/src/deep_links.rs`
- Add `src-tauri/src/tray.rs`
- Add `src-tauri/src/trace.rs` if diagnostics remain long-lived
- Update `infrastructure.md`

Tasks:

- [ ] Move window show/hide/position helpers into `windows.rs`.
- [ ] Move hotkey registration and hotkey handlers into `hotkeys.rs`.
- [ ] Move deep-link parsing/handling glue into `deep_links.rs`, keeping auth URL parsing in `auth.rs`.
- [ ] Move tray menu setup into `tray.rs`.
- [ ] Move prewarm setup into `startup.rs` or `windows.rs`, whichever reads cleaner.
- [ ] Keep `lib.rs` responsible for module declarations, `AppState`, and high-level app builder composition.

Acceptance:

- `lib.rs` reads like app composition rather than a full implementation dump.
- Window behavior has a single Rust home.
- Hotkey behavior has a single Rust home.
- Rust check/build passes.

## Sprint 6 - Supabase Function Separation

Goal: Make the rewrite Edge Function easier to audit and safer to evolve.

Why: `supabase/functions/rewrite/index.ts` owns validation, abuse prefiltering, guarded prompt construction, usage accounting, Anthropic transport, and response shaping.

Proposed files:

- Add `supabase/functions/_shared/rewriteGuard.ts`
- Add `supabase/functions/_shared/usage.ts` if shared by future functions
- Add `supabase/functions/_shared/anthropic.ts`
- Update `infrastructure.md`

Tasks:

- [ ] Move abuse patterns, scope sentinel, and guarded prompt construction into `rewriteGuard.ts`.
- [ ] Move Anthropic request/response parsing into `anthropic.ts`.
- [ ] Keep `rewrite/index.ts` as request routing and orchestration.
- [ ] Preserve response status codes and payload shapes.

Acceptance:

- Rewrite function is easier to audit top-down.
- Guard behavior and provider transport can be reviewed independently.
- Existing client behavior remains compatible.

## Suggested Order

1. Sprint 1, because it creates a clean frontend/native boundary.
2. Sprint 2, because it removes duplicated rewrite behavior with limited blast radius.
3. Sprint 3, because Settings is the largest frontend readability issue.
4. Sprint 4, because rewrite orchestration is a core native workflow.
5. Sprint 5, because `lib.rs` is important but higher risk.
6. Sprint 6, because backend separation is useful but currently less tangled than the app shell.

## Validation Checklist Per Sprint

- [ ] `infrastructure.md` updated.
- [ ] TypeScript build passes when frontend files change.
- [ ] Rust check/build passes when Tauri files change.
- [ ] Supabase function smoke checks or deploy checks run when backend files change.
- [ ] Manual app behavior checked for touched workflows.

## Definition Of Done

The separation work is complete when:

- React pages mainly render UI and call hooks/services.
- Tauri commands are thin IPC adapters.
- Rust modules have clear names matching their domain responsibilities.
- Supabase functions delegate shared guard/provider/usage logic to shared modules.
- `infrastructure.md` can guide a new agent to the right files in under a minute.
