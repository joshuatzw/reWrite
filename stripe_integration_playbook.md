# Stripe Integration Playbook — reWrite

## Philosophy

- **Never store plan details in DB** — subscription status lives in Stripe, not Supabase
- **1:1 user-to-customer mapping** — Supabase `profiles` only stores `stripe_customer_id`
- **Webhooks are the only write path** — never update subscription state from frontend callbacks
- **Stripe Checkout only** — no custom payment forms
- **Stripe Customer Portal** — no custom subscription management UI
- **Lock Stripe API version** in dashboard to prevent breaking changes
- **Always verify webhook signatures**

---

## Data Model

Supabase `profiles` stores **only the pointer**:

```sql
profiles
  user_id             uuid  (FK → auth.users)
  stripe_customer_id  text  (1:1 mapping, set once at checkout session creation)
```

Subscription status, trial state, plan details — all live in Stripe only. Nothing to sync, nothing to drift.

---

## Architecture Overview

```
reWrite Desktop (Tauri)
  → Authorization: Bearer <user-JWT>

Edge Function: rewrite
  1. Validate JWT → user_id
  2. Look up stripe_customer_id from profiles
  3. stripe.subscriptions.list({ customer, status: 'active|trialing' })
  4. Deny with 402 if none found
  5. Proxy to Anthropic

Edge Function: get-subscription-status
  JWT → stripe_customer_id → stripe.subscriptions.list() → return { status, current_period_end, trial_end }
  (called on app startup + login + post-checkout; result cached in AppState for 1 hour)

Edge Function: create-checkout-session
  JWT → look up or create Stripe customer (check by email first to avoid duplicates)
  → save stripe_customer_id to profiles immediately (before payment)
  → create Checkout session
  → return session URL to app

Edge Function: stripe-webhook
  → verify signature
  → checkout.session.completed: confirm stripe_customer_id is linked in profiles (idempotent upsert)
  → customer.subscription.deleted: no-op (status is live-fetched); optionally log or trigger email
```

---

## What Stripe Manages (Never Replicated in Supabase)

| Thing | Lives in | Supabase |
|---|---|---|
| Subscription status | Stripe | — |
| Trial period | Stripe (on the Price) | — |
| Billing interval / price | Stripe | — |
| Payment method | Stripe | — |
| Invoices | Stripe | — |
| Customer identity | Stripe | `stripe_customer_id` pointer only |

---

## Implementation Phases

### Phase 1 — Supabase + Stripe Backend

**Supabase:**
- `profiles` table: `user_id`, `stripe_customer_id` only
- Trigger: auto-create profile row on `auth.users` signup

**Modify existing `rewrite` Edge Function:**
- Replace anon-key trust with JWT validation
- Look up `stripe_customer_id`, query Stripe subscriptions live
- Return `402 { code: "trial_expired" | "subscription_required" }` if no active/trialing subscription

**New Edge Function: `get-subscription-status`:**
- JWT → `stripe_customer_id` → `stripe.subscriptions.list(...)` → return `{ status, current_period_end, trial_end }`

**New Edge Function: `create-checkout-session`:**
- Validate JWT → get user
- `stripe.customers.list({ email })` first — avoids duplicate customers
- Save `stripe_customer_id` to `profiles` immediately
- Create Checkout session (trial configured on the Stripe Price, not the session)
- Return session URL

**New Edge Function: `stripe-webhook`:**
- Verify Stripe signature on every request
- `checkout.session.completed` → idempotent upsert of `stripe_customer_id` in profiles
- `customer.subscription.deleted` → log / trigger notification (no DB subscription status to update)

---

### Phase 2 — Auth in the App

- Add `tauri-plugin-deep-link` to `Cargo.toml`
- Register `rewrite://` custom URL scheme in `tauri.conf.json`
- New `src-tauri/src/auth.rs`: load/save `AuthSession` (access + refresh tokens) to `auth.json` in app config dir
- On startup: load tokens → refresh if expired → call `get-subscription-status` → cache in `AppState` for 1 hour
- Replace hardcoded anon key in `rewrite.rs` with user `access_token`
- Deep-link handler: intercepts `rewrite://auth#access_token=...` → saves tokens → emits `auth:complete` to frontend

**New Tauri commands:**
- `get_auth_state` → `{ logged_in, subscription_status, trial_days_left }`
- `login` → opens browser to Supabase auth URL
- `logout` → clears `auth.json`
- `check_subscription` → force re-validates against Stripe via `get-subscription-status`

---

### Phase 3 — UI + Gating

**Hard gate (server-side, cannot be bypassed):**
- Edge Function `rewrite` queries Stripe live on every call
- Returns `402` with error code if not active/trialing

**Soft gate (UX layer):**
- App reads 1-hour-cached status to show upgrade modal proactively before request fires
- `rewrite_with_skill` and super-hotkey path in `lib.rs` surface `trial_expired` / `subscription_required` to frontend
- Frontend maps error codes to upgrade modal

**Settings — Account tab:**
- Email, subscription status (from cache), trial days remaining
- **Upgrade button**: `create-checkout-session` → open browser → Stripe Checkout → webhook confirms → app calls `check_subscription` to refresh cache
- **Manage subscription button**: Stripe Customer Portal URL (no custom UI)

---

## Open Decision

**Trial shape** (set on the Stripe Price in dashboard — no code change needed):
- Card-required upfront: higher intent, cleaner non-conversion
- No-card trial: lower friction, more signups
