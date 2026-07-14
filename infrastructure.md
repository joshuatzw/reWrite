# Infrastructure Map

Agents must read this file first. Keep it current whenever files are added, removed, renamed, or materially changed.

## Agent And Planning Docs

| File | Responsibility |
|---|---|
| `agent.md` | Operating rules for agents, including the requirement to start here and keep this map updated. |
| `separation-of-concerns-sprintmap.md` | Followable sprint plan for separating UI, IPC, Rust workflow, and backend concerns. |
| `roadmap.md` | Product and billing roadmap, especially Stripe, Supabase, plan limits, and SaaS rollout notes. |
| `README.md` | High-level project overview, setup instructions, broad architecture summary, and pointer to the agent/infrastructure workflow. |

## Frontend App Shell

| File | Responsibility |
|---|---|
| `src/main.tsx` | React entry point. |
| `src/App.tsx` | Routes each Tauri window label to the correct React page. |
| `src/types.ts` | Shared frontend TypeScript types for config, skills, and history. |
| `src/skills.ts` | Frontend skill metadata and skill-list shaping for overlay/menu display. |
| `src/index.css` | Global frontend styles, fonts, animations, and scroll styling. |

## Frontend Pages

| File | Responsibility |
|---|---|
| `src/pages/Overlay.tsx` | Full skill-picker overlay, captured-text preview, keyboard navigation, rewrite selection, paste flow, and limit error UI. |
| `src/pages/Bubble.tsx` | Tiny selection bubble window that forwards clicks to Rust. |
| `src/pages/BubbleMenu.tsx` | Compact skill menu shown from the selection bubble, including rewrite selection and paste flow. |
| `src/pages/Processing.tsx` | Processing/limit indicator window and its positioning behavior. |
| `src/pages/Settings.tsx` | Settings window root plus login, home dashboard, history, skills, billing, preferences, and update UI. This is a primary refactor target. |
| `src/pages/settingsComponents.tsx` | Shared Settings UI components such as sidebar, nav buttons, icons, and toggle. |
| `src/pages/settingsConstants.ts` | Settings constants such as version, accent color, free-tier limit, and built-in skill options. |
| `src/pages/settingsHelpers.ts` | Pure helpers for greetings, hotkey labels, dates, names, history grouping, streaks, and word stats. |
| `src/pages/settingsTypes.ts` | Settings-specific frontend types. |

## Frontend Assets

| Path | Responsibility |
|---|---|
| `src/assets/` | Frontend logo/image assets imported by React pages. |
| `public/` | Public static assets served by Vite. |
| `logo/` | Source logo asset folder. |

## Tauri App Shell And Windowing

| File | Responsibility |
|---|---|
| `src-tauri/src/main.rs` | Native app entry point that calls into the Tauri library setup. |
| `src-tauri/src/lib.rs` | App state, startup wiring, tray, hotkeys, prewarmed windows, deep links, selection listeners, and window helpers. This is a primary refactor target. |
| `src-tauri/tauri.conf.json` | Tauri application configuration, windows, identifiers, icons, plugins, and deep-link setup. |
| `src-tauri/capabilities/default.json` | Tauri capability permissions for frontend access to commands/plugins. |
| `src-tauri/build.rs` | Rust build script. |
| `src-tauri/Cargo.toml` | Rust crate dependencies and Tauri plugin configuration. |

## Tauri Commands And Domain Modules

| File | Responsibility |
|---|---|
| `src-tauri/src/commands.rs` | Tauri IPC command boundary for rewrite, paste, config, settings, skills, history, auth, billing, bubble, and diagnostics. This should become thinner over time. |
| `src-tauri/src/config.rs` | Config model, defaults, TOML load/save, and migration of old default hotkeys. |
| `src-tauri/src/skills.rs` | Rust skill model, built-in prompts, skill inheritance, display names, prompt composition, and skills JSON persistence. |
| `src-tauri/src/history.rs` | History model, word counting, encrypted history load/save, and legacy plaintext fallback. |
| `src-tauri/src/rewrite.rs` | Client-side transport to the Supabase rewrite Edge Function. |
| `src-tauri/src/auth.rs` | Supabase auth session persistence, token refresh, magic link, subscription sync, checkout, portal, and auth deep-link parsing. |
| `src-tauri/src/clipboard.rs` | Clipboard capture, paste, rich-text paste, restore, and HTML stripping behavior. |
| `src-tauri/src/foreground.rs` | Foreground app/output-format detection for plain text vs rich text paste. |
| `src-tauri/src/secure_store.rs` | Local encryption/decryption wrapper for persisted sensitive data. |
| `src-tauri/src/selection_watcher.rs` | Windows selection detection, source-window tracking, bubble anchoring, and outside-click support. |
| `src-tauri/src/esc_hook.rs` | Windows low-level Esc handling for overlay dismissal. |
| `src-tauri/examples/uia_probe.rs` | Diagnostic UI Automation probe example. |

## Supabase Backend

| File | Responsibility |
|---|---|
| `supabase/config.toml` | Local Supabase function config and JWT verification overrides. |
| `supabase/migrations/001_profiles.sql` | Profiles table and initial auth-user profile trigger. |
| `supabase/migrations/002_usage_limits.sql` | Usage-limit support migration. |
| `supabase/migrations/003_plan_tiers.sql` | Plan-tier usage support and atomic usage function updates. |
| `supabase/functions/_shared/cors.ts` | Shared CORS and JSON response helpers for Edge Functions. |
| `supabase/functions/_shared/plan.ts` | Shared plan resolution and monthly limit helpers. |
| `supabase/functions/rewrite/index.ts` | Authenticated rewrite endpoint, abuse/scope guard, usage accounting, Anthropic call, and result response. |
| `supabase/functions/sync-subscription/index.ts` | Refreshes local subscription cache from Stripe for the authenticated user. |
| `supabase/functions/create-checkout-session/index.ts` | Creates Stripe Checkout sessions and links Stripe customers to Supabase users. |
| `supabase/functions/create-portal-session/index.ts` | Creates Stripe billing portal sessions. |
| `supabase/functions/stripe-webhook/index.ts` | Stripe webhook signature verification and subscription cache updates. |
| `supabase/functions/checkout-success/index.ts` | Hosted checkout success redirect helper. |
| `supabase/functions/checkout-cancel/index.ts` | Hosted checkout cancellation redirect helper. |

## Build And Package

| File | Responsibility |
|---|---|
| `package.json` | Frontend scripts and npm dependencies. |
| `package-lock.json` | Locked npm dependency graph. |
| `vite.config.ts` | Vite and Tauri dev-server configuration. |
| `tsconfig.json` | TypeScript project configuration for frontend code. |
| `tsconfig.node.json` | TypeScript configuration for Node/Vite config files. |
| `index.html` | Vite HTML entry point. |
| `src-tauri/icons/` | Tauri app icons for desktop/mobile packaging. |

## Reference And Investigation Docs

| File | Responsibility |
|---|---|
| `rewrite-app-spec.md` | Product specification and expected app behavior. |
| `test-cases.md` | Manual/functional test cases. |
| `update.md` | Update notes. |
| `cicd.md` | CI/CD notes. |
| `v1.1.0-selection-bubble.md` | Selection bubble feature plan/notes. |
| `bubble-menu-bug-diagnosis.md` | Bubble menu diagnosis and trace notes. |
| `stripe-integration-practices.md` | Stripe integration reference notes. |
| `stripe_integration_playbook.md` | Stripe integration playbook. |
| `stripe_payments_best_practice.md` | Stripe payment best-practice notes. |

## Current Refactor Hotspots

| Area | Why It Matters | Start Here |
|---|---|---|
| Settings frontend | One file owns many screens, data loading, commands, and UI state. | `src/pages/Settings.tsx`, then `src/pages/settingsHelpers.ts` and `src/pages/settingsComponents.tsx` |
| Rewrite selection flow | Overlay and bubble menu duplicate skill loading, rewrite invocation, paste, and quota handling. | `src/pages/Overlay.tsx`, `src/pages/BubbleMenu.tsx`, `src/skills.ts` |
| IPC command boundary | Commands contain both IPC adapters and workflow/business orchestration. | `src-tauri/src/commands.rs`, then `src-tauri/src/rewrite.rs`, `src-tauri/src/history.rs`, `src-tauri/src/skills.rs` |
| Tauri app startup/windowing | `lib.rs` mixes app state, window control, startup, tray, hotkeys, deep links, and selection UI. | `src-tauri/src/lib.rs` |
| Supabase rewrite function | Request validation, usage accounting, prompt guarding, provider call, and response shaping live in one function. | `supabase/functions/rewrite/index.ts`, then `_shared/plan.ts` and `_shared/cors.ts` |

## Update Checklist

When changing files:

1. Update this map for every added, removed, renamed, or materially changed file.
2. Keep file responsibilities short and concrete.
3. Move a file between sections if its concern changes.
4. Add new hotspots when a file starts carrying multiple concerns.
5. Remove hotspots once the separation work is complete.
