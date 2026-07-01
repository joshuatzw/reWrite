# reWrite

**System-wide AI text transformer for Windows.**

Highlight any text, press a hotkey, pick a rewrite style — the result replaces your selection in the same app, no context switch required.

---

## What it does

reWrite sits in the system tray and listens for two global hotkeys:

| Hotkey | Action |
|---|---|
| `Ctrl+Shift+R` | Capture selected text and open the skill-picker overlay |
| `Ctrl+Shift+.` | Silently apply the default skill and paste — no overlay |

The overlay lets you choose how to rewrite (Proofread, Formal Email, Summarise, Shorten, or any custom skill you've created). The super hotkey skips the picker entirely and applies your default skill in one keystroke.

---

## Status — June 2026

### Done
- **System tray app** — starts silently, Settings and Quit in tray menu
- **Overlay hotkey** (`Ctrl+Shift+R`) — captures selection, shows skill-picker overlay
- **Super hotkey** (`Ctrl+Shift+.`) — silent one-shot rewrite with the default skill
- **Overlay UI** — keyboard-navigable skill list (arrow keys + Enter), ESC to dismiss, error state
- **4 built-in skills** — Proofread, Formal Email, Summarise, Shorten
- **Custom skills** — create/edit/delete, optional base skill (stacks prompts), toggleable, reorderable
- **Anthropic API** — raw `reqwest` call to Claude Sonnet 4.6, configurable model
- **API key storage** — Windows Credential Manager via `keyring` crate
- **Rewrite history** — persisted to `history.json`, browsable with search and skill filter
- **Settings window** — multi-view UI: Home (stats/onboarding), History, Skills, Settings
- **Config** — hotkeys, default skill, model, clipboard restore, paste delay — persisted as TOML

### UI-only / placeholder
- Launch on startup toggle (no Rust backend yet)
- Sound on rewrite toggle (no audio yet)
- Account / Plan & billing section (hardcoded placeholder for SaaS design)
- Intro video card (placeholder)

### Not started
- Phase 3 SaaS backend (hosted API key, usage metering, billing)
- Per-app tone memory
- Multi-monitor active-window centering (currently centers on primary screen)
- Hotkey conflict detection on first launch
- Token/usage counter

---

## Architecture

```
src-tauri/src/
  lib.rs          # App setup, tray, hotkey handlers, window builders
  commands.rs     # All Tauri IPC commands (rewrite, config, skills, history)
  config.rs       # Config struct + TOML load/save
  skills.rs       # Skill model, prompt composition, built-in prompts
  history.rs      # HistoryEntry, HistoryStore, JSON load/save
  rewrite.rs      # Anthropic API call (reqwest, no SDK)
  clipboard.rs    # capture_selection (Ctrl+C), paste_and_restore (Ctrl+V)
  main.rs         # Tauri entry point

src/
  App.tsx         # Routes to Overlay or Settings by window label
  pages/Overlay.tsx    # Floating skill-picker window
  pages/Settings.tsx   # Full settings app (Home / History / Skills / Settings)
```

**Key design choices:**

- **Overlay is hidden, not destroyed.** `paste_text` calls `window.hide()`; `show_overlay` re-uses the existing WebView via `get_webview_window("overlay")`. This eliminates WebView2 cold-start delay.
- **State reset on focus.** `Overlay.tsx` reloads captured text and skills on the Tauri `onFocusChanged` event rather than on mount, so stale state from the previous session is cleared each time the overlay appears.
- **Single `reqwest::Client`** lives in `AppState.http_client` (Send+Sync, no Mutex). Cloned per call, reuses TLS connections.
- **Two hotkeys.** `on_hotkey` captures text and shows the overlay. `on_super_hotkey` captures, calls the API directly with the default skill, logs to history, and pastes — no UI shown.
- **Skill prompt composition.** Global instructions + skill core prompt + optional base skill inheritance, assembled in `skills::build_system_prompt`.

---

## Config defaults

| Field | Default |
|---|---|
| `hotkey` | `ctrl+shift+r` |
| `super_hotkey` | `ctrl+shift+period` |
| `model` | `claude-sonnet-4-6` |
| `default_skill_id` | `__proofread__` |
| `restore_clipboard` | `true` |
| `restore_delay_ms` | `500` |
| `paste_delay_ms` | `400` |

Config file: `%APPDATA%\com.rewrite.app\config.toml`
Skills: `%APPDATA%\com.rewrite.app\skills.json`
History: `%APPDATA%\com.rewrite.app\history.json`

---

## Development

```bash
# Install dependencies
npm install

# Run in dev mode (hot reload for frontend; Rust recompiles on change)
npm run tauri dev

# Build release
npm run tauri build
```

**Prerequisites:** Rust toolchain, Node 18+, WebView2 runtime (ships with Windows 11).

---

## Tech stack

| Layer | Choice |
|---|---|
| App framework | Tauri 2 |
| Frontend | React + TypeScript + Vite + Tailwind CSS |
| Hotkeys | `tauri-plugin-global-shortcut` |
| Clipboard | `arboard` + `enigo` (SendInput) |
| HTTP | `reqwest` + `tokio` |
| Config | `toml` + `serde` |
| API key storage | `keyring` (Windows Credential Manager) |
| AI | Anthropic Claude (Sonnet 4.6 default) |
