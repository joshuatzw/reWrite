# reWrite — Product Specification
**System-wide AI Text Transformer for Windows**
Version 0.1 — Draft
Josh / Personal Build

---

## 1. Overview

reWrite is a system-wide hotkey-triggered AI text transformer for Windows, built with Tauri 2 + Rust. It lives in the system tray, has zero UI chrome until you need it, and does exactly one thing: takes highlighted text from any application and rewrites it in a format you choose — without making you leave your current window.

It is the "Apple Intelligence Writing Tools" experience, built for Windows, productised as a SaaS.

---

## 2. The Problem

Windows knowledge workers rewrite text dozens of times a day — switching a draft Slack message to a formal email, turning bullet notes into an exec summary, making a blunt internal message sound more diplomatic. Right now they:

1. Copy the text
2. Open ChatGPT / Claude in a browser tab
3. Paste, type an instruction, wait
4. Copy the result back
5. Paste it into the original app

This is 5 steps and a full context switch. reWrite does it in 2: **highlight → hotkey → pick a format → done.**

---

## 3. Target User

Primary: Windows-based knowledge workers who write heavily across Outlook, Teams, Slack, Notion, Jira, and browser-based tools — PMs, BD/partnerships professionals, ops leads, consultants.

Secondary: Non-native English speakers in professional roles who need fast tone/formality adjustment without visible AI scaffolding.

Not targeting: Creative writers, students, content marketers (that's a different market with Grammarly and Wordtune already owning it).

---

## 4. Core User Flow

```
User highlights text in any Windows app
         ↓
Presses hotkey: Ctrl + Shift + Space
         ↓
Lightweight command-palette overlay appears
(centred on screen, dark, ~480px wide)
         ↓
User sees format options:
  [ Formal Email ]  [ Casual / Slack ]  [ Bullet Points ]
  [ Executive Summary ]  [ Fix Grammar Only ]  [ Shorter ]
  [ Longer ]  [ Friendly ]  [ Custom... ]
         ↓
User clicks or arrows + Enter to select
         ↓
Overlay shows a subtle spinner / shimmer for ~1–2s
         ↓
Rewritten text replaces original selection in the source app
Overlay closes automatically
```

---

## 5. Hotkey

**Default: `Ctrl + Shift + Space`**

Rationale: Avoids collision with `Ctrl + Win` (reserved for WisprClone dictation), does not conflict with standard Windows system shortcuts, and is comfortable one-handed on most keyboards. User-configurable in Settings.

---

## 6. Overlay UI

The overlay is a Tauri WebView window — frameless, always-on-top, transparent background, borderless. It appears anchored to screen centre (not to cursor, to avoid awkward positioning near edges).

```
┌─────────────────────────────────────────┐
│  ✦ How should this be rewritten?        │
│                                         │
│  [ Formal Email ]    [ Casual / Slack ] │
│  [ Bullet Points ]   [ Exec Summary ]   │
│  [ Fix Grammar ]     [ Make it Shorter ]│
│  [ Make it Longer ]  [ More Friendly ]  │
│                                         │
│  ┌───────────────────────────────────┐  │
│  │  Custom instruction...            │  │
│  └───────────────────────────────────┘  │
│                                         │
│  ESC to cancel                          │
└─────────────────────────────────────────┘
```

**Design language:** Dark navy/charcoal background (#0F1117), white text, subtle border with a low-opacity glow, keyboard-navigable. Format buttons highlight on hover with a light accent. No window chrome, no taskbar entry, no title bar.

**Keyboard nav:** Arrow keys to move between options, Enter to confirm, Escape to dismiss without action.

---

## 7. Architecture — Tauri 2 + Rust

### 7.1 System Tray + Hotkey Registration

```
src-tauri/
  ├── main.rs              # Tauri app entrypoint, system tray setup
  ├── hotkey.rs            # Global hotkey listener (rdev or global-hotkey crate)
  ├── clipboard.rs         # Read selection, write rewritten text back
  ├── rewrite.rs           # Calls Anthropic API with selected format prompt
  ├── overlay.rs           # Creates/destroys the overlay WebView window
  └── config.rs            # Reads/writes user settings (hotkey, API key, presets)
```

**Global hotkey:** Use the [`global-hotkey`](https://crates.io/crates/global-hotkey) crate (Tauri-ecosystem native, avoids rdev's driver-level complexity on Windows). Register `Ctrl+Shift+Space` on app startup.

**Clipboard capture strategy:**
1. On hotkey fire: simulate `Ctrl+C` to copy current selection to clipboard (using `enigo` crate or Windows `SendInput` API directly)
2. Read clipboard text via `arboard` crate
3. After rewrite: write result to clipboard, simulate `Ctrl+V` to paste back
4. Restore original clipboard content after 500ms delay

> ⚠️ Edge case: Some apps (e.g. certain terminal emulators) don't respond to `SendInput`-triggered Ctrl+C. Log these silently and show a fallback toast: "Couldn't read selection — try copying manually first."

### 7.2 Overlay Window

```rust
// overlay.rs (pseudocode)
fn show_overlay(app: &AppHandle) {
    let window = WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("/overlay".into()))
        .decorations(false)
        .always_on_top(true)
        .transparent(true)
        .skip_taskbar(true)
        .inner_size(480.0, 320.0)
        .center()
        .build()
        .unwrap();
}
```

Frontend: React (Vite) with Tailwind. The overlay is a single-page component that receives the format selection via Tauri command invocation and emits the choice back to Rust.

### 7.3 Rewrite Engine

```rust
// rewrite.rs
async fn rewrite_text(text: &str, format: RewriteFormat) -> Result<String> {
    let system_prompt = format.system_prompt(); // maps enum → prompt string
    let client = AnthropicClient::new(config::api_key());
    client.messages()
        .model("claude-sonnet-4-6")
        .system(system_prompt)
        .user(text)
        .max_tokens(1024)
        .send()
        .await
}
```

**Format → Prompt mapping** (stored in `presets.toml`, user-editable):

| Format | System Prompt |
|---|---|
| Formal Email | "Rewrite the following as a polished, professional business email. Preserve the meaning. Return only the rewritten text." |
| Casual / Slack | "Rewrite the following in a friendly, casual tone suitable for a Slack message. Be concise. Return only the rewritten text." |
| Bullet Points | "Convert the following into clear, concise bullet points. Return only the bullets." |
| Exec Summary | "Rewrite the following as a tight executive summary in 2–4 sentences. Return only the summary." |
| Fix Grammar | "Fix any grammar, spelling, and punctuation errors in the following text. Preserve the tone and meaning exactly. Return only the corrected text." |
| Make it Shorter | "Shorten the following text while preserving its full meaning. Return only the shortened version." |
| Make it Longer | "Expand the following text with more detail and context while preserving its meaning and tone. Return only the expanded version." |
| More Friendly | "Rewrite the following in a warmer, more approachable tone. Return only the rewritten text." |

### 7.4 Config & State

User config stored at `%APPDATA%\reWrite\config.toml`:

```toml
[general]
hotkey = "Ctrl+Shift+Space"
model = "claude-sonnet-4-6"
api_key = ""  # encrypted at rest using Windows DPAPI

[behaviour]
restore_clipboard = true
restore_delay_ms = 500
show_spinner = true
auto_close_overlay_after_rewrite = true

[presets]
# User can add/edit/remove entries here
```

---

## 8. Feature Phases

### Phase 1 — MVP (build this first)
- System tray app, starts on login
- `Ctrl+Shift+Space` global hotkey
- Clipboard-based text capture (copy → read → rewrite → paste)
- Overlay with 8 preset format buttons + custom text input
- Anthropic API integration (user supplies own API key)
- ESC to dismiss
- Config: hotkey override, API key entry

### Phase 2 — Polish & Retention
- Rewrite history panel (last 20 rewrites, dismissable)
- Per-app tone memory: "always use Casual for Slack, always use Formal for Outlook"
- Token/usage counter in settings
- Onboarding flow for first-run API key entry

### Phase 3 — SaaS & Monetisation
- Replace user-supplied API key with hosted backend (reWrite API key, usage metered)
- Free tier: 30 rewrites/month
- Pro tier: unlimited rewrites, custom presets, priority processing — ~$8–10/month
- Team tier: shared brand voice presets, admin dashboard, SSO

### Phase 4 — Ecosystem
- Custom preset creator (UI for defining your own format + prompt)
- Brand voice profile: paste sample writing, the app extracts a style prompt
- Windows 11 Quick Settings toggle integration
- Optional: macOS port (overlaps with Apple Intelligence — lower priority)

---

## 9. Competitive Differentiators

| Feature | reWrite | Wispr Flow | Grammarly | Wordtune |
|---|---|---|---|---|
| Works in any Windows app | ✅ | ✅ | ❌ (browser/extension) | ❌ (browser/extension) |
| Format picker UI | ✅ | ❌ (voice command) | ❌ | ❌ |
| No browser required | ✅ | ✅ | ❌ | ❌ |
| Text-first (no voice) | ✅ | ❌ (voice primary) | ✅ | ✅ |
| Replaces in-place | ✅ | ✅ | ❌ | ❌ |
| Command-palette UX | ✅ | ❌ | ❌ | ❌ |
| Windows-native | ✅ | Partial (March '25) | ❌ | ❌ |

**Core USP:** The format chooser overlay. Every competitor either auto-rewrites silently, or makes you leave your app. reWrite gives you a fast, keyboard-driven choice of *how* — without breaking your flow.

---

## 10. Technical Dependencies

| Dependency | Role |
|---|---|
| `tauri` 2.x | App framework, WebView host, IPC |
| `global-hotkey` | System-wide hotkey registration |
| `arboard` | Cross-platform clipboard read/write |
| `enigo` | Simulate keyboard input (Ctrl+C / Ctrl+V) |
| `reqwest` + `tokio` | Async HTTP for Anthropic API calls |
| `serde` / `toml` | Config serialisation |
| `keyring` (Windows DPAPI) | Secure API key storage |
| React + Vite + Tailwind | Overlay frontend |

---

## 11. Open Questions

1. **Clipboard race condition**: There's a ~50–100ms window between simulating Ctrl+C and reading the clipboard where another process could interfere. Mitigate by polling clipboard until content changes, with a 2s timeout.

2. **Apps that block clipboard access**: Some sandboxed apps (e.g. certain password managers, secure browsers) block `SendInput`. Fallback UX needed — likely a "paste your text here" input in the overlay itself.

3. **Hotkey conflict detection**: On first launch, detect if `Ctrl+Shift+Space` is already registered by another process and prompt the user to choose an alternative.

4. **API key model vs hosted**: For MVP, user supplies own Anthropic key (zero infra cost to you). For SaaS, build a thin proxy that meters usage per account. Decide before Phase 3.

5. **Overlay position on multi-monitor setups**: Centre on the monitor containing the active window (not the primary monitor). Requires querying the foreground window's monitor via `GetMonitorInfo` Win32 API.

---

## 12. Name & Positioning

Working name: **reWrite**

Tagline candidates:
- *"Say it better. Instantly."*
- *"Any app. Any tone. One hotkey."*
- *"The rewrite button Windows never had."*

Pricing page anchor: Compare to Apple Intelligence Writing Tools — "this is that, but for Windows, with your choice of format."

---

*Spec owner: Josh — last updated June 2026*
