use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

const EDGE_FUNCTION_URL: &str =
    "https://jrzcedtyqyzfqbfuabxa.supabase.co/functions/v1/rewrite";
const SUPABASE_ANON_KEY: &str =
    "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6ImpyemNlZHR5cXl6ZnFiZnVhYnhhIiwicm9sZSI6ImFub24iLCJpYXQiOjE3ODIzNjk2ODUsImV4cCI6MjA5Nzk0NTY4NX0.Qe5HCqlmP8z--ZsI3w8uw1QtMF60udrWV7XsuT9Lay4";

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
}

pub async fn call_api_raw(
    client: &reqwest::Client,
    system: &str,
    user_message: &str,
    model: &str,
) -> Result<String> {
    let body = EdgeRequest {
        system_prompt: system.to_string(),
        user_message: user_message.to_string(),
        model: model.to_string(),
    };

    let response = client
        .post(EDGE_FUNCTION_URL)
        .header("Authorization", format!("Bearer {SUPABASE_ANON_KEY}"))
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let detail = response.text().await.unwrap_or_default();
        return Err(anyhow!("API error {status}: {detail}"));
    }

    let parsed: EdgeResponse = response.json().await?;

    if let Some(err) = parsed.error {
        return Err(anyhow!("{err}"));
    }

    parsed.text.ok_or_else(|| anyhow!("No text in response"))
}
