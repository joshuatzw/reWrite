use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

const EDGE_FUNCTION_URL: &str =
    "https://jrzcedtyqyzfqbfuabxa.supabase.co/functions/v1/rewrite";

#[derive(Serialize)]
struct EdgeRequest {
    system_prompt: String,
    user_message: String,
    model: String,
}

#[derive(Deserialize)]
struct EdgeResponse {
    text: Option<String>,
    error: Option<String>,
    rewrite_count: Option<u32>,
}

/// Result of a rewrite call: the transformed text plus the caller's updated
/// monthly usage count (as recorded server-side), when the Edge Function
/// reports it.
pub struct RewriteResult {
    pub text: String,
    pub rewrite_count: Option<u32>,
}

pub async fn call_api_raw(
    client: &reqwest::Client,
    access_token: &str,
    system: &str,
    user_message: &str,
    model: &str,
) -> Result<RewriteResult> {
    let body = EdgeRequest {
        system_prompt: system.to_string(),
        user_message: user_message.to_string(),
        model: model.to_string(),
    };

    let response = client
        .post(EDGE_FUNCTION_URL)
        .header("Authorization", format!("Bearer {access_token}"))
        .json(&body)
        .send()
        .await?;

    // Surface limit/subscription errors as typed codes the frontend can act on
    if response.status() == reqwest::StatusCode::PAYMENT_REQUIRED {
        let detail: serde_json::Value = response.json().await.unwrap_or_default();
        let code = detail
            .get("code")
            .and_then(|c| c.as_str())
            .unwrap_or("limit_reached");
        return Err(anyhow!("{code}"));
    }

    if response.status() == reqwest::StatusCode::FORBIDDEN {
        let detail: serde_json::Value = response.json().await.unwrap_or_default();
        let message = detail
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("This request is outside reWrite's scope of text rewriting, refining, and translation.");
        return Err(anyhow!("{message}"));
    }

    if !response.status().is_success() {
        let status = response.status();
        let detail = response.text().await.unwrap_or_default();
        return Err(anyhow!("API error {status}: {detail}"));
    }

    let parsed: EdgeResponse = response.json().await?;

    if let Some(err) = parsed.error {
        return Err(anyhow!("{err}"));
    }

    let text = parsed.text.ok_or_else(|| anyhow!("No text in response"))?;
    Ok(RewriteResult { text, rewrite_count: parsed.rewrite_count })
}
