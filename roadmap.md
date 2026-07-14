# reWrite — Build Roadmap

## Decisions locked in

- **Stripe is the authority** on subscription status — Supabase never calls Stripe from the rewrite hot path
- **Supabase stores a cache** (`is_subscribed`, `plan`, `subscription_valid_until`, `last_synced_at`) refreshed from Stripe on app open and every 24 hours — not on every rewrite
- **`plan` is cached too** (`'pro' | 'max' | null`), resolved from the subscription's Stripe price id via `STRIPE_PRO_PRICE_ID` / `STRIPE_MAX_PRICE_ID` — needed because Free/Pro/Max have different monthly limits (see Plans below)
- **Rewrite Edge Function reads from cache** — zero Stripe API calls in the hot path
- **Usage checks are atomic** — `check_and_increment_usage()` (Postgres function, row-locked) checks the monthly count and increments it in one step, so concurrent requests can't both read a stale count and slip past the limit
- **Webhooks are complementary** — they keep the cache fresh on cancellations/renewals between syncs, but the system is correct without them
- **Webhook JWT verification is disabled** (`supabase/config.toml`, `verify_jwt = false` for `stripe-webhook`, `checkout-success`, `checkout-cancel`) — these are hit by Stripe/the bare browser, never with a Supabase JWT, so the platform's default JWT gate would 401 them before the function code ever ran
- **Stripe Checkout + Customer Portal only** — no custom payment UI
- **1:1 user-to-customer mapping** — `stripe_customer_id` set once at checkout, never changes; the Stripe customer's `metadata.supabase_user_id` is the source of truth the webhook uses to resolve a Stripe event back to a Supabase user (there's no `getUserByEmail` on the admin API, and email isn't guaranteed unique)
- **Webhook signatures always verified** — no exceptions
- **Skills are a Pro/Max feature** — free-tier users keep the 4 built-in skills in the overlay/hotkey picker, but the Settings → Skills tab (creating custom skills, toggling built-ins) is greyed out and locked behind an upgrade prompt
- **Stripe API version locked** in dashboard before going live

---

## Plans

| Plan | Rewrites / month | Skills tab |
|---|---|---|
| Free | 3 | Locked (upgrade prompt) |
| Pro | 1,000 | Unlocked |
| Max | 5,000 | Unlocked |

---

## Data model

```sql
-- Auto-created on auth.users insert via trigger
create table profiles (
  id                      uuid primary key references auth.users,
  stripe_customer_id      text,

  -- Stripe cache (written by sync-subscription and stripe-webhook only)
  is_subscribed           boolean default false,
  plan                    text,   -- 'pro' | 'max' | null
  subscription_valid_until timestamptz,
  last_synced_at          timestamptz,

  -- Usage tracking (Stripe doesn't track per-call usage natively)
  rewrite_count           int default 0,
  rewrite_month           text   -- 'YYYY-MM', reset when month changes
);
```

Nothing else lives in Supabase regarding billing. Trial state, plan interval, payment method — all in Stripe.

---

## Architecture

```
App opens / 24h timer fires
        ↓
sync-subscription (Edge Fn)
  JWT → stripe_customer_id → stripe.subscriptions.list()
  → resolve plan from price id (STRIPE_PRO_PRICE_ID / STRIPE_MAX_PRICE_ID)
  → UPDATE profiles SET is_subscribed, plan, subscription_valid_until, last_synced_at
        ↓
Subscription status cached in AppState (in-memory)

User rewrites text
        ↓
rewrite (Edge Fn)
  JWT → read profiles (is_subscribed, plan)
  → monthly limit = 3 (free) / 1,000 (pro) / 5,000 (max)
  → check_and_increment_usage(user, month, limit) — atomic, row-locked
  → allowed  → call Anthropic, return result
  → blocked  → return 402 { code: "limit_reached" }

Stripe event fires (subscription renewed / cancelled)
        ↓
stripe-webhook (Edge Fn)
  verify signature
  → resolve plan from price id, UPDATE profiles (same fields as sync-subscription)
  → idempotent, no other business logic
```

---

## Build phases

### Before go-live (production keys)

- [ ] Swap `STRIPE_SECRET_KEY` from `sk_test_...` to live key in Supabase secrets
- [ ] Swap `STRIPE_PRO_PRICE_ID` and `STRIPE_MAX_PRICE_ID` to live price IDs
- [ ] Swap `STRIPE_WEBHOOK_SECRET` to the live endpoint's signing secret
- [ ] Set `CHECKOUT_SUCCESS_URL` and `CHECKOUT_CANCEL_URL` to the hosted redirect pages (see below)
- [ ] Lock Stripe API version in dashboard
- [x] `supabase/config.toml` with `verify_jwt = false` for `stripe-webhook` / `checkout-success` / `checkout-cancel` — deployed

---

### Hosted redirect pages (need to build)

Stripe Checkout requires HTTPS `success_url` and `cancel_url` — custom URL schemes like `rewrite://` are not valid there. You need two simple hosted pages that immediately bounce back to the app.

**`/checkout/success`** — shown after successful payment
- Displays: "You're all set! Returning to reWrite…"
- On load: `window.location.href = 'rewrite://checkout-success'`
- Deep-link handler in app triggers `sync_subscription` then emits `auth:complete`

**`/checkout/cancel`** — shown if user closes Stripe checkout
- Displays: "No worries — you can upgrade anytime from reWrite."
- On load: `window.location.href = 'rewrite://checkout-cancelled'`
- Deep-link handler in app: no-op (user just abandoned checkout)

**`/auth/callback`** *(optional, for future OAuth login)*
- If you add Google login later, OAuth redirects need an HTTPS callback page
- Page exchanges the code, then bounces to `rewrite://auth#access_token=...`

**Where to host:** Any static host works — your existing website, Vercel, or a Supabase Storage public bucket. The pages are ~10 lines of HTML each. (Currently `checkout-success` / `checkout-cancel` are served directly as Supabase Edge Functions — fine for now, but worth moving to a static host before go-live per the note above.)

**Set in Supabase secrets once pages are live:**
```
CHECKOUT_SUCCESS_URL = https://yoursite.com/checkout/success
CHECKOUT_CANCEL_URL  = https://yoursite.com/checkout/cancel
```

---

### Phase 1 — Supabase backend

- [x] Create `profiles` table with trigger (`supabase/migrations/001_profiles.sql`)
- [x] Add `plan` column + atomic usage RPC (`supabase/migrations/002_usage_limits.sql`, superseded by `003_plan_tiers.sql`)
- [x] New Edge Function: `sync-subscription`
  - Validates JWT
  - Fetches `stripe_customer_id` from profiles
  - Calls `stripe.subscriptions.list({ customer, status: ['active', 'trialing'] })`
  - Resolves `plan` from the subscription's price id
  - Updates `is_subscribed`, `plan`, `subscription_valid_until`, `last_synced_at`
  - Returns `{ is_subscribed, subscription_valid_until, trial_end, rewrite_count }`
- [x] Modify existing Edge Function: `rewrite`
  - Replace anon-key trust with JWT validation
  - Read subscription status + plan from `profiles` cache (not Stripe)
  - Gate all users via `check_and_increment_usage()` — 3 (free) / 1,000 (pro) / 5,000 (max) per month
  - Cap input size at 20,000 characters regardless of plan
  - Return `402 { code: "limit_reached" }` on deny
- [x] New Edge Function: `create-checkout-session`
  - `stripe.customers.list({ email })` first to avoid duplicate customers
  - Stamps `metadata.supabase_user_id` onto the Stripe customer either way (new or existing)
  - Upsert `stripe_customer_id` into profiles
  - Create Stripe Checkout session with `success_url` pointing to `rewrite://checkout-success`
  - Return session URL
- [x] New Edge Function: `create-portal-session`
  - JWT → `stripe_customer_id` → `stripe.billingPortal.sessions.create()`
  - Return portal URL
- [x] New Edge Function: `stripe-webhook`
  - Verify Stripe signature (reject if invalid)
  - Handle: `checkout.session.completed`, `customer.subscription.created/updated/deleted`
  - Resolves the Supabase user via Stripe customer metadata (not email lookup)
  - Idempotent upsert of cache fields (`is_subscribed`, `plan`, `subscription_valid_until`) in profiles

---

### Phase 2 — Auth in the Tauri app

- [x] `tauri-plugin-deep-link` in `Cargo.toml`, `rewrite://` scheme registered in `tauri.conf.json`
- [x] `src-tauri/src/auth.rs`
  - `AuthSession` struct: `access_token`, `refresh_token`, `expires_at`
  - `load_session()` / `save_session()` — persisted to `auth.json` in app config dir
  - `refresh_session()` — calls Supabase `/auth/v1/token?grant_type=refresh_token`
- [x] Extended `AppState` in `lib.rs` — `auth_session: Mutex<Option<AuthSession>>`, `subscription: Mutex<SubscriptionCache>`
- [x] On startup in `lib.rs` setup block — load + refresh auth session, `sync_subscription`, 24h background re-sync timer
- [x] Deep-link handler — `rewrite://auth#...`, `rewrite://checkout-success`, `rewrite://checkout-cancelled`
- [x] `rewrite.rs` uses the user's `access_token` from AppState (no more anon-key trust)
- [x] Tauri commands: `get_auth_state`, `send_magic_link`, `logout`, `open_checkout`, `open_billing_portal`, `refresh_subscription`

---

### Phase 3 — UI wiring

- [x] Login screen (`LoginView` in `Settings.tsx`) — magic-link email flow, shown when no valid session
- [x] Settings → Settings view — real `email`/`is_subscribed` from `get_auth_state`, plan badge, "Manage billing" / "Upgrade to Pro" / "Upgrade to Max" buttons, `X / 3 rewrites used this month` for free users
- [x] Settings → Skills tab locked for free users — greyed-out nav item + lock icon, upsell card in place of the skill editor
- [x] Overlay / rewrite error handling — `402 { code: "limit_reached" }` currently surfaces as a generic error string in the overlay, not a dedicated "upgrade to continue" prompt. Still open.
- [ ] Super-hotkey path (`on_super_hotkey`) — does not yet soft-gate on cached subscription state before firing; a blocked request only surfaces once the Edge Function rejects it. Still open.

---

## Open decisions

| Decision | Options | Default |
|---|---|---|
| Trial shape | Card required upfront vs. no-card trial | No-card (lower friction for desktop app) |
| Login method | Magic link only / Google OAuth / both | Magic link (simplest) |
| 402 UX | Inline overlay error / separate modal window | Inline overlay error (see Phase 3, still needs the "upgrade" messaging) |


**NEW FEATURES**

2. Default Skills Change:
Proofread -- Proofreads and fixes grammatical errors, spellings before you send them out. Retains your writing style as is. 
#Prompt for Proofread: 
Correct all spelling, grammar, and punctuation errors in the text below. 
Do not change the writer's tone, vocabulary, sentence structure, or word choice unless it contains an actual error. 
Do not rephrase for style, do not shorten or lengthen it, and do not make it more formal or casual.
Preserve line breaks, formatting, and paragraph structure exactly as given.
[IMPORTANT] Return only the corrected text, with no explanation or commentary.


Polish -- Refines your current text to make it suitable for a third party to review it. polishes the language to make it professional.
#Prompt for Polish: 
Rewrite the text below so it is ready to be shared with a third party (e.g. a colleague, client, or manager) for review.
Fix any grammar or clarity issues, tighten loose phrasing, and adjust tone so it reads as professional and considered.
Keep the length roughly the same — do not summarize or expand significantly.
Preserve the core meaning, intent, and key details exactly. Do not add new claims, arguments, or information.
[IMPORTANT] Return only the rewritten text, with no explanation or commentary.



Summarise -- Summarises a long thought or chunk, elevating the best bits so your message gets across
#Prompt for Summarise:
Summarize the text below, keeping only the most important points, decisions, or asks.
Preserve the original intent and any critical details (numbers, names, deadlines, action items) — do not lose information that changes the meaning.
Write in clear, complete sentences (not just fragments or bullet-only unless the input is already a list).
Aim for roughly 30-50% of the original length, adjusting based on how much can be safely cut.
[IMPORTANT] Return only the summary, with no explanation or commentary.


Enhance (Beef it up) - Feel that the writing is too thin? rewrite enhances your email/proposal/executive summary so that it sounds polished and ready to go
#Prompt for Enhance:
The text below feels thin or underdeveloped. Rewrite it to be more substantial and persuasive, suitable for a polished email, proposal, or executive summary.
Add depth by strengthening weak statements, making vague points more concrete, and improving the logical flow between ideas — but do not invent specific facts, numbers, or claims that aren't implied by the original.
Elevate the language and structure so it reads as complete and ready to send, without becoming bloated or repetitive.
[IMPORTANT] Return only the rewritten text, with no explanation or commentary.


Here is 


**BUGS:**
[ ] Change the supabase Auth sign in link - laymen won't know.
[ ] Verify bubble-menu HiDPI clamp on real hardware (Windows @ 125/150/200% scale).
    `show_bubble_menu` now clamps via `clamp_to_monitor(&w, ...)`, which reads
    `outer_size()` right after `set_size(...)`. This assumes `set_size` is
    applied synchronously before the size is read back — true on the main
    thread today (SetWindowPos / setFrame are synchronous), but the menu can be
    resized to 44x44 during the loading spinner, so a stale read would size the
    clamp wrong near a screen edge. Confirm the menu never hangs off the
    right/bottom edge on a scaled multi-monitor setup. (See lib.rs
    show_bubble_menu + bubble-menu-bug-diagnosis.md.)

**Security and Abuse**

