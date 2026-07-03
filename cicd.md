# CI/CD — GitHub Actions macOS Build Checklist

## Verdict: very feasible — but "build for macOS" is two jobs, not one

The **CI/CD mechanics are easy** and we're already ~80% there. GitHub Actions gives
free macOS runners (Apple Silicon by default), and `tauri-apps/tauri-action` — which
`release.yml` already uses — supports a build matrix out of the box. The updater
signing key is already wired up (`TAURI_SIGNING_PRIVATE_KEY`), which is the annoying
part for most people.

The **real work is making the Rust actually compile and run on macOS.** The app is
Windows-only in practice today. Concrete blockers found:

- **`secure_store.rs` won't even compile on macOS.** Declared `pub mod secure_store;`
  in `lib.rs:18` *without* a `cfg` gate, but has bare `use windows_sys::...Cryptography`
  imports (DPAPI). Hard compile error on a Mac runner.
- **`esc_hook.rs` is gated** (`lib.rs:20`) so it compiles — but that means ESC-to-dismiss
  doesn't exist on macOS at all.
- **Windows-native runtime behaviors**: DPAPI at-rest encryption (auth.json/history),
  global-shortcut capture, `enigo` paste, and the HTML clipboard path all need macOS
  equivalents *and* macOS **Accessibility permission** (TCC) — which only sticks
  reliably if the app is code-signed.
- **`foreground.rs` macOS path** (NSWorkspace/objc2) was written but never compiled on a Mac.

CI is ~a day. A *shippable, signed, working* macOS build is a small porting project.

---

## Checklist

### Phase 0 — Decisions & accounts
- [x] **Apple Developer Program** ($99/yr). Required for signing + notarization. Without it,
      Gatekeeper blocks users *and* Accessibility permission won't persist across updates.
- [x] Target arch decided: **Universal** (`universal-apple-darwin`, covers Intel + Apple Silicon).
- [x] Billing noted: repo stays **private** for security → macOS runners bill at **10x minutes**.

### Phase 1 — Make it compile on macOS
- [ ] **Fix `secure_store.rs`** — the hard blocker. Either:
  - gate the DPAPI impl with `#[cfg(target_os = "windows")]` + add a macOS impl
    (Keychain via `security-framework`), **or**
  - add a cross-platform fallback (plaintext or simple file-based) behind the same
    `encrypt`/`decrypt` signature so callers don't change.
- [ ] Audit every other bare `windows_sys` use for a gate (`esc_hook.rs` done; double-check
      `commands.rs:180/197`, `lib.rs:96/727`).
- [ ] Get a green `cargo check --target aarch64-apple-darwin` — do this in CI first (Phase 3)
      since there's no Mac locally.

### Phase 2 — Make it actually work at runtime (verify on real macOS)
- [ ] **Verify `foreground.rs` macOS path** (NSWorkspace/objc2) — currently unverified.
- [ ] **Accessibility permission flow**: paste (`enigo`) + global shortcuts require the user
      to grant Accessibility in System Settings. Add a first-run prompt/instructions.
- [ ] **ESC-to-dismiss**: `esc_hook` doesn't exist on macOS — add a mac hook or handle ESC in
      the overlay's JS/Tauri layer as a fallback.
- [ ] **HTML clipboard paste** (`clipboard::paste_html_and_restore`) — confirm arboard's
      `set().html()` behaves on macOS.
- [ ] **Deep-link scheme** `rewrite://` registration on macOS (Info.plist via Tauri config).
- [ ] Tray icon + window pre-warm behavior (Windows pre-warm gotcha is Windows-specific — recheck on mac).

### Phase 3 — CI workflow (matrix build)
- [ ] Convert `release.yml` to a matrix adding a macOS runner:
  ```yaml
  strategy:
    matrix:
      include:
        - platform: windows-latest
          args: ""
        - platform: macos-latest
          args: "--target universal-apple-darwin"
  runs-on: ${{ matrix.platform }}
  ```
- [ ] Add `rustup target add aarch64-apple-darwin x86_64-apple-darwin` step (needed for universal).
- [ ] Pass `args: ${{ matrix.args }}` to `tauri-action`.
- [x] Early win DONE: **`cargo check` on macOS on every PR/push to main** →
      `.github/workflows/macos-check.yml` (checks both aarch64 + x86_64; no installers/signing;
      does not touch the Windows `release.yml`). This is the feedback loop for Phase 1.

### Phase 4 — Code signing & notarization
- [ ] Create a **Developer ID Application** certificate; export as `.p12`.
- [ ] Add repo secrets for `tauri-action`: `APPLE_CERTIFICATE` (base64 .p12),
      `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`,
      `APPLE_PASSWORD` (app-specific password), `APPLE_TEAM_ID`.
- [ ] Confirm notarization succeeds (tauri-action runs `notarytool` when those env vars are present).

### Phase 5 — Auto-updater cross-platform
- [ ] Updater endpoint (`latest.json`) already works; `tauri-action` adds the `darwin-aarch64` /
      `darwin-x86_64` entries automatically once mac artifacts build.
- [ ] Same `TAURI_SIGNING_PRIVATE_KEY` signs mac update bundles — no new key needed.
- [ ] Verify a Mac client can see and apply an update.

---

## Suggested path
Knock out Phase 1 + a `cargo check` matrix job first (cheap, gets a compiling Mac binary
via CI without owning a Mac), *then* tackle signing and runtime porting once there's a
`.app` to actually test.

## Reference
- Current workflow: `.github/workflows/release.yml` (Windows-only, tag-triggered `v*`)
- Tauri config: `src-tauri/tauri.conf.json` (updater pubkey + endpoint already set, `icon.icns` bundled)
- macOS deps already declared: `Cargo.toml` `[target.'cfg(target_os = "macos")'.dependencies]`
