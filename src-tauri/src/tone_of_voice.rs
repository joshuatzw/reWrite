use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::auth::{SUPABASE_ANON_KEY, SUPABASE_URL};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToneOfVoice {
    pub id: String,
    pub name: String,
    pub content: String,
    pub is_default: bool,
}

fn base_url() -> String {
    format!("{SUPABASE_URL}/rest/v1/tone_of_voice")
}

/// Current UTC time as an ISO-8601 / RFC-3339 string (e.g. `2026-07-03T12:34:56Z`),
/// derived from the system clock without pulling in a date-time crate. Uses Howard
/// Hinnant's civil-from-days algorithm to split the epoch second into Y-M-D.
fn now_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hour, min, sec) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    // civil_from_days: days since 1970-01-01 -> (year, month, day)
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };

    format!(
        "{year:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z"
    )
}

/// GET all of the caller's tones, oldest-first.
pub async fn list_tones(client: &reqwest::Client, token: &str) -> Result<Vec<ToneOfVoice>> {
    let resp = client
        .get(base_url())
        .query(&[
            ("select", "id,name,content,is_default"),
            ("order", "created_at.asc"),
        ])
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(anyhow!("list_tones failed: {}", resp.status()));
    }

    let tones: Vec<ToneOfVoice> = resp.json().await?;
    Ok(tones)
}

/// POST a new tone and return the created row.
pub async fn create_tone(
    client: &reqwest::Client,
    token: &str,
    name: &str,
    content: &str,
) -> Result<ToneOfVoice> {
    let resp = client
        .post(base_url())
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("Prefer", "return=representation")
        .json(&serde_json::json!({ "name": name, "content": content }))
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(anyhow!("create_tone failed: {}", resp.status()));
    }

    let mut rows: Vec<ToneOfVoice> = resp.json().await?;
    if rows.is_empty() {
        return Err(anyhow!("create_tone returned no row"));
    }
    Ok(rows.remove(0))
}

/// PATCH an existing tone's name + content.
pub async fn update_tone(
    client: &reqwest::Client,
    token: &str,
    id: &str,
    name: &str,
    content: &str,
) -> Result<()> {
    let now = now_iso8601();
    let resp = client
        .patch(base_url())
        .query(&[("id", format!("eq.{id}"))])
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "name": name,
            "content": content,
            "updated_at": now,
        }))
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(anyhow!("update_tone failed: {}", resp.status()));
    }
    Ok(())
}

/// DELETE a tone by id.
pub async fn delete_tone(client: &reqwest::Client, token: &str, id: &str) -> Result<()> {
    let resp = client
        .delete(base_url())
        .query(&[("id", format!("eq.{id}"))])
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(anyhow!("delete_tone failed: {}", resp.status()));
    }
    Ok(())
}

/// Call the set_default_tone RPC, which atomically flips is_default to this id.
pub async fn set_default_tone(client: &reqwest::Client, token: &str, id: &str) -> Result<()> {
    let resp = client
        .post(format!("{SUPABASE_URL}/rest/v1/rpc/set_default_tone"))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({ "p_id": id }))
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(anyhow!("set_default_tone failed: {}", resp.status()));
    }
    Ok(())
}
