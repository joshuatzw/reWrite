# reWrite — Build Roadmap

## Decisions locked in

- **Stripe is the authority** on subscription status — no plan columns, no tier logic in Supabase
- **Supabase stores a cache** (`is_subscribed`, `subscription_valid_until`, `last_synced_at`) refreshed from Stripe on app open and every 24 hours — not on every rewrite
- **Rewrite Edge Function reads from cache** — zero Stripe API calls in the hot path
- **Webhooks are complementary** — they keep the cache fresh on cancellations/renewals between syncs, but the system is correct without them
- **Stripe Checkout + Customer Portal only** — no custom payment UI
- **1:1 user-to-customer mapping** — `stripe_customer_id` set once at checkout, never changes
- **Webhook signatures always verified** — no exceptions
- **Stripe API version locked** in dashboard before going live

---

## Data model

```sql
-- Auto-created on auth.users insert via trigger
create table profiles (
  id                      uuid primary key references auth.users,
  stripe_customer_id      text,

  -- Stripe cache (written by sync-subscription and stripe-webhook only)
  is_subscribed           boolean default false,
  subscription_valid_until timestamptz,
  last_synced_at          timestamptz,

  -- Free-tier usage (Stripe doesn't track per-call usage natively)
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
  → UPDATE profiles SET is_subscribed, subscription_valid_until, last_synced_at
        ↓
Subscription status cached in AppState (in-memory)

User rewrites text
        ↓
rewrite (Edge Fn)
  JWT → read profiles (is_subscribed, rewrite_count, rewrite_month)
  → subscribed?      → call Anthropic, return result
  → not subscribed + count < 30? → call Anthropic, increment rewrite_count
  → not subscribed + count >= 30? → return 402 { code: "limit_reached" }

Stripe event fires (subscription renewed / cancelled)
        ↓
stripe-webhook (Edge Fn)
  verify signature
  → UPDATE profiles (same fields as sync-subscription)
  → idempotent, no business logic
```

---

## Build phases

### Before go-live (production keys)

- [ ] Swap `STRIPE_SECRET_KEY` from `sk_test_...` to live key in Supabase secrets
- [ ] Swap `STRIPE_PRO_PRICE_ID` and `STRIPE_MAX_PRICE_ID` to live price IDs
- [ ] Swap `STRIPE_WEBHOOK_SECRET` to the live endpoint's signing secret
- [ ] Set `CHECKOUT_SUCCESS_URL` and `CHECKOUT_CANCEL_URL` to the hosted redirect pages (see below)
- [ ] Lock Stripe API version in dashboard

---

### Hosted redirect pages (need to build)

Stripe Checkout requires HTTPS `success_url` and `cancel_url` — custom URL schemes like `rewrite://` are not valid there. You need two simple hosted pages that immediately bounce back to the app.

**`/checkout/success`** — shown after successful payment
- Displays: "You're all set! Returning to reWrite…"
- On load: `window.location.href = 'rewrite://checkout-success'`
- Deep-link handler in app triggers `refresh_subscription` then emits `subscription:updated`

**`/checkout/cancel`** — shown if user closes Stripe checkout
- Displays: "No worries — you can upgrade anytime from reWrite."
- On load: `window.location.href = 'rewrite://checkout-cancelled'`
- Deep-link handler in app: no-op (user just abandoned checkout)

**`/auth/callback`** *(optional, for future OAuth login)*
- If you add Google login later, OAuth redirects need an HTTPS callback page
- Page exchanges the code, then bounces to `rewrite://auth#access_token=...`

**Where to host:** Any static host works — your existing website, Vercel, or a Supabase Storage public bucket. The pages are ~10 lines of HTML each.

**Set in Supabase secrets once pages are live:**
```
CHECKOUT_SUCCESS_URL = https://yoursite.com/checkout/success
CHECKOUT_CANCEL_URL  = https://yoursite.com/checkout/cancel
```

---

### Phase 1 — Supabase backend

- [x] Create `profiles` table with trigger (`supabase/migrations/001_profiles.sql`)
- [x] New Edge Function: `sync-subscription`
  - Validates JWT
  - Fetches `stripe_customer_id` from profiles
  - Calls `stripe.subscriptions.list({ customer, status: ['active', 'trialing'] })`
  - Updates `is_subscribed`, `subscription_valid_until`, `last_synced_at`
  - Returns `{ is_subscribed, subscription_valid_until, trial_end, rewrite_count }`
- [x] Modify existing Edge Function: `rewrite`
  - Replace anon-key trust with JWT validation
  - Read subscription status from `profiles` cache (not Stripe)
  - Gate free users at 30 rewrites/month (reset `rewrite_count` when `rewrite_month` changes)
  - Return `402 { code: "limit_reached" | "subscription_required" }` on deny
- [x] New Edge Function: `create-checkout-session`
  - `stripe.customers.list({ email })` first to avoid duplicate customers
  - Upsert `stripe_customer_id` into profiles
  - Create Stripe Checkout session with `success_url` pointing to `rewrite://checkout-success`
  - Return session URL
- [x] New Edge Function: `create-portal-session`
  - JWT → `stripe_customer_id` → `stripe.billingPortal.sessions.create()`
  - Return portal URL
- [x] New Edge Function: `stripe-webhook`
  - Verify Stripe signature (reject if invalid)
  - Handle: `checkout.session.completed`, `customer.subscription.updated`, `customer.subscription.deleted`
  - Idempotent upsert of cache fields in profiles
  - No business logic — write Stripe data as-is

---

### Phase 2 — Auth in the Tauri app

- [ ] Add `tauri-plugin-deep-link` to `Cargo.toml`, register `rewrite://` URL scheme in `tauri.conf.json`
- [ ] New `src-tauri/src/auth.rs`
  - `AuthSession` struct: `access_token`, `refresh_token`, `expires_at`
  - `load_session()` / `save_session()` — persisted to `auth.json` in app config dir
  - `refresh_if_expired()` — calls Supabase `/auth/v1/token?grant_type=refresh_token`
- [ ] Extend `AppState` in `lib.rs`
  - Add `auth_session: Mutex<Option<AuthSession>>`
  - Add `subscription: Mutex<SubscriptionCache>` (is_subscribed, valid_until, rewrite_count, synced_at)
- [ ] On startup in `lib.rs` setup block
  - Load + refresh auth session
  - If logged in: call `sync-subscription`, populate `subscription` in AppState
  - Start 24h background timer to re-call `sync-subscription`
- [ ] Deep-link handler: intercepts `rewrite://auth#access_token=...&refresh_token=...`
  - Saves tokens to `auth.json`
  - Calls `sync-subscription` to populate subscription cache
  - Emits `auth:complete` event to settings window
- [ ] Replace hardcoded anon key in `rewrite.rs` with user `access_token` from AppState
- [ ] New Tauri commands
  - `get_auth_state` → `{ logged_in: bool, email: String, is_subscribed: bool, subscription_valid_until: Option<i64>, rewrite_count: u32 }`
  - `login` → opens browser to Supabase magic-link / OAuth URL
  - `logout` → clears `auth.json`, clears AppState session
  - `open_checkout` → calls `create-checkout-session`, opens browser to returned URL
  - `open_billing_portal` → calls `create-portal-session`, opens browser to returned URL
  - `refresh_subscription` → force re-calls `sync-subscription`, updates AppState

---

### Phase 3 — UI wiring

**Login screen** (new window or inline in settings):
- Shown on app open if no valid session
- Email input → magic link flow (or Google OAuth button)
- After deep-link callback → auto-close login, open settings

**Settings → Settings view** (replace placeholders):
- Account section: real `email` from `get_auth_state`
- Plan & billing section:
  - Subscribed: show plan badge, `subscription_valid_until` date, "Manage billing" button → `open_billing_portal`
  - Free tier: show rewrite count `X / 30 this month`, "Upgrade to Pro" button → `open_checkout`
  - Trial: show "Trial — X days left", same upgrade button

**Overlay / rewrite error handling**:
- `rewrite_with_skill` in `commands.rs` maps `402` error codes:
  - `limit_reached` → emit event to show upgrade modal in overlay
  - `subscription_required` → same
- Overlay shows inline "You've hit your monthly limit — upgrade to continue" with button

**Super-hotkey path** (`on_super_hotkey` in `lib.rs`):
- Check `subscription` in AppState before firing (soft gate, no network call)
- If blocked: show a brief toast notification instead of the processing spinner

---

## Open decisions

| Decision | Options | Default |
|---|---|---|
| Trial shape | Card required upfront vs. no-card trial | No-card (lower friction for desktop app) |
| Trial length | 7 / 14 / 30 days | 14 days |
| Free tier limit | 10 / 20 / 30 rewrites/month | 30 |
| Pro price | $8 / $10 / $12/month | $10 |
| Login method | Magic link only / Google OAuth / both | Magic link (simplest) |
| 402 UX | Inline overlay error / separate modal window | Inline overlay error |
