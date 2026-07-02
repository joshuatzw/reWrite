# Shipping a new version (OTA update)

Read this before every release — it tells you what to check and in what order.

## 0. One-time setup (already done, just for reference)

- Signing keypair generated with `npx tauri signer generate`.
- Public key lives in `src-tauri/tauri.conf.json` under `plugins.updater.pubkey`.
- Private key + its password are stored as GitHub Actions repo secrets:
  `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
  (Settings → Secrets and variables → Actions on the GitHub repo).
- If you ever lose these secrets, old installs can no longer verify new updates —
  you'd need to generate a new keypair, update `pubkey` in `tauri.conf.json`, and
  every user would need to reinstall manually once (last time OTA works with the old key).

## 1. Check what version you're currently on

Three files carry a version number and **all three must match** before you tag a release:

- `src-tauri/tauri.conf.json` → `"version"` (this is the one the running app reports —
  it's what `getVersion()` shows in Settings, and what the updater compares against)
- `package.json` → `"version"`
- `src-tauri/Cargo.toml` → `[package] version`

Run this to see all three at once:

```bash
grep -n version src-tauri/tauri.conf.json package.json src-tauri/Cargo.toml
```

Also check the latest published release so you don't collide with a version already out:

```bash
git tag --list | sort -V | tail -5
```

## 2. Decide the next version number

Standard semver — pick based on what actually changed since the last release:

- **Patch** (`0.1.0` → `0.1.1`): bug fixes, no new features, no behavior changes users would notice
- **Minor** (`0.1.0` → `0.2.0`): new features, backward-compatible
- **Major** (`0.1.0` → `1.0.0`): breaking changes (rare for a desktop app like this — usually only for things like config format changes that require migration)

If unsure, skim `git log <last-tag>..HEAD --oneline` to see what's actually shipping.

## 3. Bump the version in all three files

Update to the **exact same number** in:

1. `src-tauri/tauri.conf.json` → `"version"`
2. `package.json` → `"version"`
3. `src-tauri/Cargo.toml` → `version = "..."`

Do not add a `v` prefix inside these files — plain semver only (e.g. `0.1.1`, not `v0.1.1`).

## 4. Commit the version bump

```bash
git add src-tauri/tauri.conf.json package.json src-tauri/Cargo.toml
git commit -m "chore: bump version to 0.1.1"
```

## 5. Tag and push

The tag **must** be the version number prefixed with `v` — this is what triggers
`.github/workflows/release.yml`.

```bash
git tag v0.1.1
git push origin main
git push origin v0.1.1
```

## 6. Let the release workflow run

Pushing the tag kicks off the `Release` GitHub Action. It:

- Builds the Windows installer (NSIS/MSI)
- Signs it with the private key from the repo secrets
- Generates `latest.json` (the manifest the updater endpoint reads)
- Publishes everything as a **draft** GitHub Release named after the tag

Watch it at `github.com/joshuatzw/reWrite/actions`. It takes a few minutes.

## 7. Review and publish the release

1. Go to `github.com/joshuatzw/reWrite/releases`
2. Find the new **draft** release
3. Confirm it has the installer file(s) *and* `latest.json` attached
4. Add release notes if you want (optional — not required for the updater to work)
5. Click **Publish release**

Nothing is live for existing users until you publish — the draft is invisible to the
updater endpoint, which reads `.../releases/latest/...`.

## 8. Verify the update actually works

On a machine (or VM) running the *previous* version:

1. Open reWrite → Settings
2. Click **Check for updates**
3. It should report downloading, install, and relaunch itself at the new version
4. Confirm the version shown in Settings now matches what you just published

New installs also auto-check once in the background shortly after launch (release
builds only — this is skipped in `npm run tauri dev`), so a fresh app open should also
pick it up without the button.

## If something goes wrong

- **Bad release published**: unpublish it (or delete it) on GitHub — the updater
  endpoint always points at `releases/latest`, so removing/unpublishing a broken
  release stops it from being offered. Then ship a fixed patch version through the
  steps above.
- **Workflow fails to sign**: almost always means the `TAURI_SIGNING_PRIVATE_KEY` /
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets are missing or wrong — check
  Settings → Secrets and variables → Actions on the repo.
- **Version already tagged**: you can't reuse a tag. Bump to the next patch number
  instead of retagging.
