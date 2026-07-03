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
- [x] **Fixed `secure_store.rs`** — cfg-gated impls behind unchanged `encrypt`/`decrypt`:
      Windows = DPAPI (unchanged); macOS = **AES-256-GCM with a per-install key in the login
      Keychain** (keyring `apple-native` + aes-gcm + rand); other = passthrough (dev/CI).
- [x] **Fixed `lib.rs` `transparent` errors** (105/126/714/782) — enabled tauri `macos-private-api`
      feature in `Cargo.toml` + `"macOSPrivateApi": true` in `tauri.conf.json`.
      NOTE: bars Mac App Store submission (fine for Developer ID distribution).
- [x] Windows `cargo check` still green locally after the refactor (58s).
- [x] **Green `cargo check` for both arches in CI** (PR #1, run 28661327576, 3m8s) — macOS-only
      code (Keychain/AES + private-api) compiles on real Mac runners. **Phase 1 compiles.**

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
- [x] **`release.yml` converted to a Windows+macOS matrix** (branch `ci/macos-release-matrix`).
      - macOS runner builds `--target universal-apple-darwin`; rust targets added via the
        toolchain action per-matrix (`rust-targets`).
      - `args: ${{ matrix.args }}` wired to tauri-action; `fail-fast: false` so one OS failing
        doesn't kill the other.
      - Added `workflow_dispatch` (input `tag`, default `manual-test`) → produces a **draft**
        release for pipeline testing without cutting a real `v*` tag.
      - Apple signing env vars wired but **inert until secrets are set** (unsigned build still
        produces artifacts).
- [x] Early win DONE: **`cargo check` on macOS on every PR/push to main** →
      `.github/workflows/macos-check.yml` (both arches; no installers/signing).
- [ ] Validate the matrix produces a macOS bundle: run **Actions → Release → Run workflow**
      (manual draft build) once merged, before relying on a real tag.

### Phase 4 — Code signing & notarization
Secrets to add under **repo → Settings → Secrets and variables → Actions**. The workflow
already references all of these; adding them flips the macOS build from unsigned → notarized.

- [ ] **Developer ID Application** cert. If you have a Mac: create it in Keychain Access
      (Certificate Assistant → request from a CA) + Apple Developer portal, then export the
      cert **with its private key** as `.p12`. (No Mac = harder; needs a CSR — do this on a
      borrowed Mac or a cloud Mac.)
- [ ] `APPLE_CERTIFICATE` = base64 of the `.p12`:
      - macOS/Linux: `base64 -i cert.p12 | pbcopy`
      - Windows PowerShell: `[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.p12")) | Set-Clipboard`
- [ ] `APPLE_CERTIFICATE_PASSWORD` = the password set when exporting the `.p12`.
- [ ] `APPLE_SIGNING_IDENTITY` = e.g. `Developer ID Application: Your Name (TEAMID)`.
- [ ] `APPLE_ID` = your Apple account email.
- [ ] `APPLE_PASSWORD` = an **app-specific password** (appleid.apple.com → Sign-In & Security),
      NOT your real password.
- [ ] `APPLE_TEAM_ID` = 10-char team id from the developer portal.
- [ ] Confirm notarization succeeds (tauri-action runs `notarytool` automatically once the
      APPLE_* vars are present).

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
