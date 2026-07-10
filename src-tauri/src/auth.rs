use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const SUPABASE_URL: &str = "https://jrzcedtyqyzfqbfuabxa.supabase.co";
pub const SUPABASE_ANON_KEY: &str =
    "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6ImpyemNlZHR5cXl6ZnFiZnVhYnhhIiwicm9sZSI6ImFub24iLCJpYXQiOjE3ODIzNjk2ODUsImV4cCI6MjA5Nzk0NTY4NX0.Qe5HCqlmP8z--ZsI3w8uw1QtMF60udrWV7XsuT9Lay4";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSession {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub email: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SubscriptionCache {
    pub is_subscribed: bool,
    pub plan: Option<String>,
    pub subscription_valid_until: Option<String>,
    pub rewrite_count: u32,
    pub synced_at: Option<i64>,
}

// ── Disk I/O ──────────────────────────────────────────────────────────────────

pub fn load_session(path: &Path) -> Option<AuthSession> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() {
        return None;
    }

    // Preferred path: DPAPI-encrypted JSON.
    if let Some(plain) = crate::secure_store::decrypt(&bytes) {
        if let Ok(session) = serde_json::from_slice::<AuthSession>(&plain) {
            return Some(session);
        }
    }

    // Legacy fallback: raw plaintext JSON written before at-rest encryption.
    serde_json::from_slice::<AuthSession>(&bytes).ok()
}

pub fn save_session(session: &AuthSession, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(session)?;
    let cipher = crate::secure_store::encrypt(&json)?;
    std::fs::write(path, cipher)?;
    Ok(())
}

pub fn clear_session(path: &Path) {
    let _ = std::fs::remove_file(path);
}

// ── Time ─────────────────────────────────────────────────────────────────────

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn is_expired(session: &AuthSession) -> bool {
    session.expires_at - now_secs() < 300
}

// ── Token refresh ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RefreshResponse {
    access_token: String,
    refresh_token: String,
    expires_at: Option<i64>,
    expires_in: Option<i64>,
}

pub async fn refresh_session(
    client: &reqwest::Client,
    session: AuthSession,
) -> Result<AuthSession> {
    let resp = client
        .post(format!(
            "{SUPABASE_URL}/auth/v1/token?grant_type=refresh_token"
        ))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({ "refresh_token": session.refresh_token }))
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(anyhow!("Token refresh failed: {}", resp.status()));
    }

    let r: RefreshResponse = resp.json().await?;
    let expires_at = r
        .expires_at
        .unwrap_or_else(|| now_secs() + r.expires_in.unwrap_or(3600));

    Ok(AuthSession {
        access_token: r.access_token,
        refresh_token: r.refresh_token,
        expires_at,
        email: session.email,
    })
}

// ── Subscription sync ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SyncResponse {
    is_subscribed: bool,
    plan: Option<String>,
    subscription_valid_until: Option<String>,
    rewrite_count: Option<u32>,
}

pub async fn sync_subscription(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<SubscriptionCache> {
    let resp = client
        .post(format!("{SUPABASE_URL}/functions/v1/sync-subscription"))
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(anyhow!("sync-subscription failed: {}", resp.status()));
    }

    let data: SyncResponse = resp.json().await?;

    Ok(SubscriptionCache {
        is_subscribed: data.is_subscribed,
        plan: data.plan,
        subscription_valid_until: data.subscription_valid_until,
        rewrite_count: data.rewrite_count.unwrap_or(0),
        synced_at: Some(now_secs()),
    })
}

// ── Magic link ────────────────────────────────────────────────────────────────

pub async fn send_magic_link(client: &reqwest::Client, email: &str) -> Result<()> {
    let resp = client
        .post(format!("{SUPABASE_URL}/auth/v1/otp"))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Content-Type", "application/json")
        .query(&[("redirect_to", "rewrite://auth")])
        .json(&serde_json::json!({ "email": email }))
        .send()
        .await?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Magic link failed: {text}"));
    }

    Ok(())
}

// ── User info ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct UserResponse {
    email: Option<String>,
}

pub async fn get_user_email(client: &reqwest::Client, access_token: &str) -> Result<String> {
    let resp = client
        .get(format!("{SUPABASE_URL}/auth/v1/user"))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await?;

    let user: UserResponse = resp.json().await?;
    user.email
        .ok_or_else(|| anyhow!("No email in user response"))
}

// ── Checkout / portal ─────────────────────────────────────────────────────────

pub async fn create_checkout_url(
    client: &reqwest::Client,
    access_token: &str,
    plan: &str,
) -> Result<String> {
    #[derive(Deserialize)]
    struct Resp {
        url: Option<String>,
    }

    let resp = client
        .post(format!(
            "{SUPABASE_URL}/functions/v1/create-checkout-session"
        ))
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({ "plan": plan }))
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(anyhow!("Checkout session failed: {}", resp.status()));
    }

    let r: Resp = resp.json().await?;
    r.url.ok_or_else(|| anyhow!("No URL in checkout response"))
}

pub async fn create_portal_url(client: &reqwest::Client, access_token: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Resp {
        url: Option<String>,
    }

    let resp = client
        .post(format!("{SUPABASE_URL}/functions/v1/create-portal-session"))
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(anyhow!("Portal session failed: {}", resp.status()));
    }

    let r: Resp = resp.json().await?;
    r.url.ok_or_else(|| anyhow!("No URL in portal response"))
}

// ── Deep-link parsing ─────────────────────────────────────────────────────────

/// Extracts access_token, refresh_token, expires_at from a `rewrite://auth#...` URL.
pub fn parse_auth_url(url: &str) -> Option<(String, String, i64)> {
    let fragment = url.split('#').nth(1)?;

    let mut access_token = None;
    let mut refresh_token = None;
    let mut expires_at: Option<i64> = None;
    let mut expires_in: Option<i64> = None;

    for pair in fragment.split('&') {
        let mut kv = pair.splitn(2, '=');
        let key = kv.next()?;
        let val = kv.next().unwrap_or("");
        match key {
            "access_token" => access_token = Some(val.to_string()),
            "refresh_token" => refresh_token = Some(val.to_string()),
            "expires_at" => expires_at = val.parse().ok(),
            "expires_in" => expires_in = val.parse().ok(),
            _ => {}
        }
    }

    let expires_at = expires_at.unwrap_or_else(|| now_secs() + expires_in.unwrap_or(3600));

    Some((access_token?, refresh_token?, expires_at))
}
