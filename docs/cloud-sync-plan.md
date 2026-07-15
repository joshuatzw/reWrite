# Cloud Sync for Skills, History, Streaks & Word Counts

**Status:** Implemented in code; migration deployment and cross-device/RLS verification pending
**Owner:** Codex implementation pass
**Last updated:** 2026-07-15

## 1. Goal

Users have asked that their **skills**, **words-rewritten totals**, **day streaks**, and
**history** persist across devices, tied to their login account.

This spec implements that by syncing to Supabase (Postgres), keyed by the stable
account UUID `auth.uid()` (this is the "auth.id" — do **not** key on email, which can
change). Row-Level Security (RLS) enforces per-user isolation.

### Two product decisions already made (do not revisit without sign-off)

1. **History syncs as METADATA ONLY.** The cloud stores each entry's skill, timestamp,
   and word count — **never the input or output text**. The rewritten prose stays on the
   device that produced it (still DPAPI-encrypted locally, unchanged). This keeps user
   content off the server.
2. **Cloud sync is available to ALL logged-in users** — not gated behind a paid plan.

### UX consequence to accept up front

On a **second** device, synced history rows render with skill name, date, and word
count, but the input/output body is **blank** (that text never left the first device).
**Streaks and word totals are fully correct on every device** because they derive only
from timestamps and per-entry word counts. The History view must show a subtle note for
text-less rows (e.g. "text stays on the original device").

## 2. What syncs, and why

| Data | Cloud scope | Rationale |
|---|---|---|
| **Skills** (`skills.json` → `SkillsConfig`) | **Full** — instructions, `global_instructions`, `builtin_enabled` map, and default skill id | User configuration (not rewritten prose); must sync fully to work cross-device |
| **History** | **Metadata only**: `id`, `timestamp_ms`, `skill_id`, `skill_name`, `output_word_count`. **No `input_text` / `output_text`.** | Privacy decision above |
| **Day streak** | **Derived, not stored** | `computeStreak` in `src/pages/settingsHelpers.ts:52` needs only timestamps |
| **Words rewritten** | **Derived, not stored** | `computeWordStats` in `src/pages/settingsHelpers.ts:85` needs only `output_word_count` + timestamps |

Because streaks and word totals are derived client-side from synced history metadata,
**no separate stats tables are needed.**

## 3. Current-state reference (read before building)

Local, single-device today. Key files:

- **History store:** `src-tauri/src/history.rs` — `HistoryEntry`, `HistoryStore`, load/save
  (DPAPI-encrypted JSON at `app_config_dir()/history.json`).
- **History write paths (TWO — both must push to cloud):**
  - `src-tauri/src/commands.rs:17` — `log_history(...)` (overlay/normal rewrite path)
  - `src-tauri/src/lib.rs:1359` — inline entry build in the super-hotkey path
  - Centralize these into one shared helper as part of this work.
- **Skills store:** `src-tauri/src/skills.rs` — `SkillsConfig`, load/save (plain JSON at
  `app_config_dir()/skills.json`). Saved via `save_skills_config` and friends in
  `commands.rs` (~line 532 onward).
- **Streak / word math (frontend, keep as-is):** `src/pages/settingsHelpers.ts` —
  `computeStreak`, `computeWordStats`. Consumed in `src/pages/Settings.tsx:141`.
- **Auth / Supabase constants:** `src-tauri/src/auth.rs` — `SUPABASE_URL`,
  `SUPABASE_ANON_KEY`, `AuthSession { access_token, refresh_token, expires_at, email }`,
  and `ensure_valid_token(&app)` (used in `commands.rs:168` to get a fresh access token).
- **Existing PostgREST-with-RLS pattern to copy:** the retired `tone_of_voice.rs` module
  (removed 2026-07-15) used exactly this approach — user `access_token` as Bearer + anon
  `apikey` against `/rest/v1/...`. Mirror it.
- **Migrations:** `supabase/migrations/` (latest is `005_lock_down_definer_functions.sql`).
  Existing RLS example: `001_profiles.sql`.

**Confirmed:** there is **no** clear/delete-history path anywhere in the app. History is
append-only and immutable → merges are conflict-free and need no tombstones.

## 4. Database migration

Add `supabase/migrations/006_cloud_sync.sql`:

```sql
-- ── Per-user skills/config blob (maps 1:1 to the Rust SkillsConfig struct) ──────
create table public.user_skills (
  user_id     uuid primary key references auth.users on delete cascade,
  config      jsonb not null,
  updated_at  timestamptz not null default now()
);

alter table public.user_skills enable row level security;

create policy "own skills"
  on public.user_skills
  for all
  using (auth.uid() = user_id)
  with check (auth.uid() = user_id);

-- ── Append-only history METADATA (deliberately NO input/output text columns) ────
create table public.rewrite_history (
  id                text primary key,          -- client-generated (nanosecond hex)
  user_id           uuid not null references auth.users on delete cascade,
  timestamp_ms      bigint not null,
  skill_id          text not null,
  skill_name        text not null,
  output_word_count int not null default 0,
  created_at        timestamptz not null default now()
);

alter table public.rewrite_history enable row level security;

create policy "own history read"
  on public.rewrite_history
  for select
  using (auth.uid() = user_id);

create policy "own history insert"
  on public.rewrite_history
  for insert
  with check (auth.uid() = user_id);

create index rewrite_history_user_ts
  on public.rewrite_history (user_id, timestamp_ms desc);
```

**Notes for the DB reviewer:**
- No `update`/`delete` policies on `rewrite_history` — append-only by design.
- `id` is the client's existing per-entry id (`skills::new_id()`, nanosecond hex). Keep
  using it as the primary key so local and cloud ids match and union-merge is trivial.
- `user_skills` uses `for all` (select/insert/update) because the blob is upserted.
- The implemented migration also adds a small `before update` trigger that keeps
  the row with the greatest `updated_at`, so delayed out-of-order requests cannot
  replace a newer skills edit with an older one.
- Do **not** add text columns to `rewrite_history` later without re-opening the privacy
  decision in §1.

## 5. Client — new module `src-tauri/src/sync.rs`

A best-effort PostgREST client. **All calls are non-blocking and failure-tolerant:** the
local files remain the source of truth, so the app must work fully offline. Never block a
rewrite or a settings save on a sync round-trip; fire sync after the local write succeeds.

Copy the request shape from the old `tone_of_voice.rs`: base URL
`{SUPABASE_URL}/rest/v1/<table>`, headers `apikey: SUPABASE_ANON_KEY`,
`Authorization: Bearer <access_token>`, `Content-Type: application/json`. Obtain a fresh
token via `crate::ensure_valid_token(&app)` (async) exactly as `rewrite_with_skill` does.

### Functions

```rust
// History metadata: one row per rewrite. NO text fields.
pub async fn push_history_meta(
    client: &reqwest::Client,
    access_token: &str,
    entry: &crate::history::HistoryEntry,   // read only id/timestamp/skill/word_count
) -> anyhow::Result<()>;
// POST /rest/v1/rewrite_history  with header `Prefer: resolution=ignore-duplicates`
// Body: { id, timestamp_ms, skill_id, skill_name, output_word_count }
// (user_id is set by RLS/default? No — send it, or rely on a column default of auth.uid().
//  Simplest: include it from the session. See note below.)

// Pull this account's history metadata and merge into the local store by id.
pub async fn pull_history_meta(
    client: &reqwest::Client,
    access_token: &str,
) -> anyhow::Result<Vec<CloudHistoryMeta>>;
// GET /rest/v1/rewrite_history?select=id,timestamp_ms,skill_id,skill_name,output_word_count&order=timestamp_ms.desc

// Skills blob upsert (last-write-wins by updated_at).
pub async fn push_skills(
    client: &reqwest::Client,
    access_token: &str,
    config: &crate::skills::SkillsConfig,
) -> anyhow::Result<()>;
// POST /rest/v1/user_skills  with header `Prefer: resolution=merge-duplicates`
// Body: { user_id, config: <SkillsConfig as JSON>, updated_at: now }

pub async fn pull_skills(
    client: &reqwest::Client,
    access_token: &str,
) -> anyhow::Result<Option<(crate::skills::SkillsConfig, i64 /*updated_at_ms*/)>>;
// GET /rest/v1/user_skills?select=config,updated_at
```

**`user_id` handling:** the cleanest option is to send `user_id` explicitly in insert/upsert
bodies (derive it from the current session — add the user's uuid to `AuthSession`, or fetch
once via `auth.rs::get_user_email`'s sibling call to `/auth/v1/user` which also returns `id`).
RLS `with check (auth.uid() = user_id)` will reject any mismatch, so this is safe. Add an
`id` (uuid) field to `AuthSession` during this work — several sync paths want it.

### Wiring / triggers

1. **On each new rewrite (both paths):** after the existing local `history::save`, call
   `push_history_meta`. Refactor `commands.rs:17 log_history` and the inline block at
   `lib.rs:1359` into **one** shared helper (e.g. `history::append_and_sync`) that (a) pushes
   to the local `HistoryStore`, (b) saves locally, (c) spawns the cloud push. Fire-and-forget
   with `tauri::async_runtime::spawn`; log failures, never surface to the user.
2. **On skills save:** in the `save_skills_config` / `create_skill` / `delete_skill` /
   `reorder_skills` / `toggle_builtin_skill` commands (`commands.rs` ~532–630), after the
   local `skills::save`, spawn `push_skills`.
3. **On login success and on app startup with a valid session:** run a `sync_all` that pulls
   skills + history metadata and merges (see §6). Hook this where the session is established
   (deep-link auth handler and the startup session-load in `lib.rs`, near the
   `subscription`/`sync_subscription` bootstrap). Emit a Tauri event when the merge changes
   local data so the UI reloads (see §7).

Register the new commands/module in `lib.rs` (add `mod sync;`) and thread the shared
`http_client` from `AppState`.

## 6. Merge logic

- **History (union by `id`):** load local `HistoryStore`; for each pulled cloud row whose
  `id` is not already local, insert a `HistoryEntry` with the metadata filled in and
  `input_text` / `output_text` set to empty strings (`String::new()`) and the real
  `output_word_count`. Conversely, push any local entries missing from the cloud. Append-only
  + immutable ids ⇒ no conflicts, no deletes, no tombstones. Save the merged store locally.
- **Skills (last-write-wins by `updated_at`):** on startup pull the blob; if the cloud
  `updated_at` is newer than the local skills file's mtime (or a stored `skills_updated_at`),
  adopt the cloud config and save locally; otherwise push the local config up. Every local
  save pushes with a fresh `updated_at`. The conflict window is tiny (skills are edited only
  in Settings), so LWW is acceptable — do **not** over-engineer with per-field merge.

## 7. Frontend changes

- **History view (`src/pages/Settings.tsx`, history panel + `settingsHelpers.ts`
  `groupByDate`):** render entries whose `input_text` / `output_text` are empty gracefully —
  show skill name, date, and word count, plus a muted note that the text lives on the
  originating device. Do not crash or show empty quotes.
- **Live refresh:** after a sync merge changes local history/skills, the Rust side emits a
  Tauri event (e.g. `history:updated` / `skills:updated`). Add listeners that re-invoke
  `get_history` / `get_skills_config` so streak, word stats, and lists update without a
  restart. (`usage:updated` at `commands.rs:203` is an existing precedent for this pattern.)
- Streak and word-count widgets need **no logic change** — they already derive from the
  history array (`Settings.tsx:141-142`).

## 8. Edge cases & behavior

1. **Offline / sync failure:** local files stay authoritative; every cloud call is
   best-effort. On the next launch/login, `sync_all` reconciles (union pull + push of
   anything the server is missing). No retry queue required for v1 — the union re-push on
   next `sync_all` covers dropped pushes.
2. **Multiple accounts on one machine:** the local `history.json` / `skills.json` are not
   user-scoped today. To avoid cross-contamination, only **push** entries/config while
   logged in as the current account, and always **pull/scope by `auth.uid()`** (RLS enforces
   this server-side regardless). Treat local files as a per-machine cache. Flag to product if
   stricter per-user local partitioning is wanted later.
3. **Logout:** keep local files as the cache (do not wipe). Next login re-pulls and merges
   that account's cloud data. Confirm this is desired before shipping.
4. **New account, existing local data:** first `sync_all` pushes local history metadata +
   skills up, so a user who used the app before logging in doesn't lose their streak.

## 9. Testing / verification

- **RLS isolation:** with two accounts, confirm account B cannot read account A's
  `rewrite_history` or `user_skills` (direct PostgREST call with B's token returns zero A
  rows). This is the security-critical check.
- **Cross-device streak:** rewrite on device A, log in on device B (fresh install), confirm
  the streak and "words rewritten" totals match A, and history rows appear with correct
  skill/date/word-count but empty text + the device-local note.
- **Skills round-trip:** create/edit/reorder/disable skills on A; confirm they appear on B
  after login, and that a later edit on B wins (LWW).
- **Offline:** disconnect network, rewrite several times, reconnect, relaunch → confirm those
  entries reach the cloud on next `sync_all`.
- **No-text guarantee:** inspect the `rewrite_history` table and verify **no** column ever
  contains input/output prose.
- Run `cargo build` / `cargo clippy` for the Rust side; exercise the app per the repo's
  verify flow.

## 10. Build order (suggested)

1. `006_cloud_sync.sql` — apply to a dev project first; verify RLS with two test users.
2. Add user `id` (uuid) to `AuthSession` (`auth.rs`) and populate it on login/session load.
3. `src-tauri/src/sync.rs` — the four functions + `CloudHistoryMeta` type; `mod sync;` in
   `lib.rs`.
4. Refactor the two history write paths into one shared `append_and_sync` helper; wire
   `push_history_meta`.
5. Wire `push_skills` into the skills-mutating commands.
6. `sync_all` on login + startup, with merge logic (§6) and `*:updated` events.
7. Frontend: empty-text rendering + event listeners for live refresh.
8. Test per §9.

## 11. Explicitly out of scope

- Syncing input/output history **text** (privacy decision — do not add text columns).
- Gating sync behind paid plans (decision: all logged-in users).
- New edge functions (direct PostgREST + RLS is sufficient).
- Real-time/websocket sync, per-field skill merge, retry queues, delete/clear-history.
