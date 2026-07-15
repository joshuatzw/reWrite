use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

use crate::{auth, history::HistoryEntry, skills::SkillsConfig, AppState};

#[derive(Debug, Clone, Deserialize)]
pub struct CloudHistoryMeta {
    pub id: String,
    pub timestamp_ms: i64,
    pub skill_id: String,
    pub skill_name: String,
    pub output_word_count: u32,
}

/// A deliberately separate wire type is the privacy boundary: HistoryEntry's
/// input_text/output_text fields cannot accidentally enter a cloud payload.
#[derive(Serialize)]
struct HistoryMetaInsert<'a> {
    id: &'a str,
    user_id: &'a str,
    timestamp_ms: i64,
    skill_id: &'a str,
    skill_name: &'a str,
    output_word_count: u32,
}

#[derive(Deserialize)]
struct CloudSkillsRow {
    config: SkillsConfig,
    updated_at: String,
}

async fn require_success(response: reqwest::Response, operation: &str) -> Result<()> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().await.unwrap_or_default();
    Err(anyhow!("{operation} failed ({status}): {body}"))
}

pub async fn push_history_meta(
    client: &reqwest::Client,
    access_token: &str,
    user_id: &str,
    entry: &HistoryEntry,
) -> Result<()> {
    let body = HistoryMetaInsert {
        id: &entry.id,
        user_id,
        timestamp_ms: entry.timestamp_ms,
        skill_id: &entry.skill_id,
        skill_name: &entry.skill_name,
        output_word_count: entry.output_word_count,
    };
    let response = client
        .post(format!("{}/rest/v1/rewrite_history", auth::SUPABASE_URL))
        .header("apikey", auth::SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .header("Prefer", "resolution=ignore-duplicates")
        .timeout(Duration::from_secs(15))
        .json(&body)
        .send()
        .await?;
    require_success(response, "history metadata push").await
}

pub async fn pull_history_meta(
    client: &reqwest::Client,
    access_token: &str,
    user_id: &str,
) -> Result<Vec<CloudHistoryMeta>> {
    const PAGE_SIZE: usize = 1000;
    let mut all_rows = Vec::new();
    loop {
        let response = client
            .get(format!("{}/rest/v1/rewrite_history", auth::SUPABASE_URL))
            .header("apikey", auth::SUPABASE_ANON_KEY)
            .header("Authorization", format!("Bearer {access_token}"))
            .timeout(Duration::from_secs(15))
            .query(&[
                (
                    "select",
                    "id,timestamp_ms,skill_id,skill_name,output_word_count".to_string(),
                ),
                ("user_id", format!("eq.{user_id}")),
                ("order", "timestamp_ms.desc,id.asc".to_string()),
                ("limit", PAGE_SIZE.to_string()),
                ("offset", all_rows.len().to_string()),
            ])
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("history metadata pull failed ({status}): {body}"));
        }
        let page = response.json::<Vec<CloudHistoryMeta>>().await?;
        let page_len = page.len();
        all_rows.extend(page);
        if page_len < PAGE_SIZE {
            break;
        }
    }
    Ok(all_rows)
}

pub async fn push_skills(
    client: &reqwest::Client,
    access_token: &str,
    user_id: &str,
    config: &SkillsConfig,
    updated_at_ms: i64,
) -> Result<()> {
    let updated_at = DateTime::<Utc>::from_timestamp_millis(updated_at_ms)
        .ok_or_else(|| anyhow!("invalid skills updated_at_ms: {updated_at_ms}"))?
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    let response = client
        .post(format!("{}/rest/v1/user_skills", auth::SUPABASE_URL))
        .header("apikey", auth::SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .header("Prefer", "resolution=merge-duplicates")
        .timeout(Duration::from_secs(15))
        .json(&serde_json::json!({
            "user_id": user_id,
            "config": config,
            "updated_at": updated_at,
        }))
        .send()
        .await?;
    require_success(response, "skills push").await
}

pub async fn pull_skills(
    client: &reqwest::Client,
    access_token: &str,
    user_id: &str,
) -> Result<Option<(SkillsConfig, i64)>> {
    let response = client
        .get(format!("{}/rest/v1/user_skills", auth::SUPABASE_URL))
        .header("apikey", auth::SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {access_token}"))
        .timeout(Duration::from_secs(15))
        .query(&[
            ("select", "config,updated_at".to_string()),
            ("user_id", format!("eq.{user_id}")),
            ("limit", "1".to_string()),
        ])
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("skills pull failed ({status}): {body}"));
    }
    let row = response.json::<Vec<CloudSkillsRow>>().await?.pop();
    row.map(|row| {
        let updated_at_ms = DateTime::parse_from_rfc3339(&row.updated_at)
            .with_context(|| format!("invalid cloud skills timestamp: {}", row.updated_at))?
            .timestamp_millis();
        Ok((row.config, updated_at_ms))
    })
    .transpose()
}

/// Gets a valid token plus the stable auth UUID. Sessions from older app
/// versions are upgraded via /auth/v1/user once and re-persisted encrypted.
async fn current_identity(app: &AppHandle) -> Result<(reqwest::Client, String, String)> {
    let access_token = crate::ensure_valid_token(app)
        .await
        .ok_or_else(|| anyhow!("not logged in"))?;
    let (client, mut session) = {
        let state = app
            .try_state::<AppState>()
            .ok_or_else(|| anyhow!("app state unavailable"))?;
        let session = state
            .auth_session
            .lock()
            .unwrap()
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow!("not logged in"))?;
        (state.http_client.clone(), session)
    };

    if session.user_id.is_empty() {
        let user = auth::get_user(&client, &access_token).await?;
        session.user_id = user.id;
        if let Some(email) = user.email {
            session.email = email;
        }
        if let Some(state) = app.try_state::<AppState>() {
            let mut current = state.auth_session.lock().unwrap();
            if current.as_ref().map(|value| value.access_token.as_str())
                != Some(access_token.as_str())
            {
                return Err(anyhow!("auth session changed during identity upgrade"));
            }
            *current = Some(session.clone());
            drop(current);
            if let Ok(path) = app.path().app_config_dir().map(|d| d.join("auth.json")) {
                auth::save_session(&session, &path)?;
            }
        }
    }

    Ok((client, access_token, session.user_id))
}

pub fn spawn_push_history(app: AppHandle, entry: HistoryEntry) {
    tauri::async_runtime::spawn(async move {
        let result = async {
            let (client, token, user_id) = current_identity(&app).await?;
            push_history_meta(&client, &token, &user_id, &entry).await
        }
        .await;
        if let Err(error) = result {
            crate::trace(&format!("cloud sync: history push skipped/failed: {error}"));
        }
    });
}

pub fn spawn_push_skills(app: AppHandle, config: SkillsConfig, updated_at_ms: i64) {
    tauri::async_runtime::spawn(async move {
        let result = async {
            let (client, token, user_id) = current_identity(&app).await?;
            push_skills(&client, &token, &user_id, &config, updated_at_ms).await
        }
        .await;
        if let Err(error) = result {
            crate::trace(&format!("cloud sync: skills push skipped/failed: {error}"));
        }
    });
}

/// Best-effort account reconciliation. Call only from a background task.
pub async fn sync_all(app: &AppHandle) -> Result<()> {
    let (client, access_token, user_id) = current_identity(app).await?;
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| anyhow!("app state unavailable"))?;

    let history_entries_to_push = match pull_history_meta(&client, &access_token, &user_id).await {
        Ok(cloud_entries) => {
            let cloud_ids: HashSet<String> =
                cloud_entries.iter().map(|row| row.id.clone()).collect();
            let (entries_to_push, changed) = {
                // Keep the mutex through save so an append cannot land between
                // a stale clone and assignment and then be overwritten.
                let mut local = state.history.lock().unwrap();
                let local_ids: HashSet<String> =
                    local.entries.iter().map(|entry| entry.id.clone()).collect();
                let mut changed = false;
                for row in cloud_entries {
                    if !local_ids.contains(&row.id) {
                        local.entries.push(HistoryEntry {
                            id: row.id,
                            timestamp_ms: row.timestamp_ms,
                            skill_id: row.skill_id,
                            skill_name: row.skill_name,
                            input_text: String::new(),
                            output_text: String::new(),
                            output_word_count: row.output_word_count,
                        });
                        changed = true;
                    }
                }
                if changed {
                    let path = app.path().app_config_dir()?.join("history.json");
                    crate::history::save(&local, &path)?;
                }
                (local.entries.clone(), changed)
            };
            if changed {
                let _ = app.emit("history:updated", ());
            }

            entries_to_push
                .iter()
                .filter(|entry| !cloud_ids.contains(entry.id.as_str()))
                .cloned()
                .collect()
        }
        Err(error) => {
            crate::trace(&format!("cloud sync: history pull failed: {error}"));
            Vec::new()
        }
    };

    let pulled_skills = pull_skills(&client, &access_token, &user_id).await;
    let mut skills_changed = false;
    let pending_push = match pulled_skills {
        Ok(cloud_row) => {
            // Serialize the decision and local persistence with command saves;
            // all snapshots are taken after the network pull returns.
            let _write = state.skills_write_lock.lock().unwrap();
            let skills_path = app.path().app_config_dir()?.join("skills.json");
            let local_updated_at_ms = crate::skills::load_updated_at_ms(&skills_path);
            let local_config = state.skills_config.lock().unwrap().clone();
            match cloud_row {
                Some((cloud_config, cloud_updated_at_ms))
                    if cloud_updated_at_ms > local_updated_at_ms =>
                {
                    crate::skills::save(&cloud_config, &skills_path)?;
                    crate::skills::save_updated_at_ms(&skills_path, cloud_updated_at_ms)?;

                    let config_path = app.path().app_config_dir()?.join("config.toml");
                    let mut app_config = state.config.lock().unwrap().clone();
                    app_config.default_skill_id = cloud_config.default_skill_id.clone();
                    crate::config::save(&app_config, &config_path)?;

                    *state.skills_config.lock().unwrap() = cloud_config;
                    *state.config.lock().unwrap() = app_config;
                    skills_changed = true;
                    None
                }
                _ => {
                    let timestamp = if local_updated_at_ms == 0 {
                        let timestamp = crate::skills::next_updated_at_ms(&skills_path);
                        crate::skills::save(&local_config, &skills_path)?;
                        crate::skills::save_updated_at_ms(&skills_path, timestamp)?;
                        timestamp
                    } else {
                        local_updated_at_ms
                    };
                    Some((local_config, timestamp))
                }
            }
        }
        Err(error) => {
            crate::trace(&format!("cloud sync: skills pull failed: {error}"));
            None
        }
    };
    if skills_changed {
        let _ = app.emit("skills:updated", ());
    }
    if let Some((config, updated_at_ms)) = pending_push {
        if let Err(error) =
            push_skills(&client, &access_token, &user_id, &config, updated_at_ms).await
        {
            crate::trace(&format!(
                "cloud sync: skills reconcile push failed: {error}"
            ));
        }
    }

    // Replay can be large, so it runs only after the single skills LWW
    // decision instead of delaying cross-device skill availability.
    for entry in &history_entries_to_push {
        if let Err(error) = push_history_meta(&client, &access_token, &user_id, entry).await {
            crate::trace(&format!(
                "cloud sync: history reconcile push failed: {error}"
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_cloud_payload_can_never_contain_prose() {
        let entry = HistoryEntry {
            id: "id".into(),
            timestamp_ms: 1,
            skill_id: "skill".into(),
            skill_name: "Skill".into(),
            input_text: "private input".into(),
            output_text: "private output".into(),
            output_word_count: 2,
        };
        let json = serde_json::to_value(HistoryMetaInsert {
            id: &entry.id,
            user_id: "user-id",
            timestamp_ms: entry.timestamp_ms,
            skill_id: &entry.skill_id,
            skill_name: &entry.skill_name,
            output_word_count: entry.output_word_count,
        })
        .unwrap();

        assert!(json.get("input_text").is_none());
        assert!(json.get("output_text").is_none());
        assert!(!json.to_string().contains("private"));
    }
}
