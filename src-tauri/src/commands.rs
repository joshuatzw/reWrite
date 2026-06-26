use std::sync::{Mutex, MutexGuard};
use tauri::{AppHandle, Manager, State, WebviewWindow};

use crate::AppState;

fn lock<T>(m: &Mutex<T>) -> Result<MutexGuard<'_, T>, String> {
    m.lock().map_err(|e| e.to_string())
}

fn history_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|d| d.join("history.json"))
        .map_err(|e| e.to_string())
}

fn log_history(
    app: &AppHandle,
    state: &AppState,
    skill_id: &str,
    input_text: &str,
    output_text: &str,
) {
    let skill_name = {
        let sc = state.skills_config.lock().unwrap();
        crate::skills::skill_display_name(&sc, skill_id)
    };
    let entry = crate::history::HistoryEntry {
        id: crate::skills::new_id(),
        timestamp_ms: crate::history::now_ms(),
        skill_id: skill_id.to_string(),
        skill_name,
        input_text: input_text.to_string(),
        output_text: output_text.to_string(),
        output_word_count: crate::history::count_words(output_text),
    };
    if let Ok(path) = history_path(app) {
        if let Ok(mut h) = state.history.lock() {
            h.entries.push(entry);
            let _ = crate::history::save(&h, &path);
        }
    }
}

// ── Overlay commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_captured_text(state: State<AppState>) -> Option<String> {
    state.captured_text.lock().unwrap().clone()
}

#[tauri::command]
pub async fn paste_text(
    result: String,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (original, paste_delay_ms, restore, restore_delay_ms) = {
        let original = lock(&state.original_clipboard)?.clone().unwrap_or_default();
        let cfg = lock(&state.config)?;
        (original, cfg.paste_delay_ms, cfg.restore_clipboard, cfg.restore_delay_ms)
    };

    window.hide().map_err(|e| e.to_string())?;
    tokio::time::sleep(tokio::time::Duration::from_millis(paste_delay_ms)).await;

    tokio::task::spawn_blocking(move || {
        crate::clipboard::paste_and_restore(&result, &original, restore, restore_delay_ms)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

// ── Skill rewrite command ─────────────────────────────────────────────────────

#[tauri::command]
pub async fn rewrite_with_skill(
    skill_id: Option<String>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<String, String> {
    let text = lock(&state.captured_text)?
        .clone()
        .ok_or("No text captured. Highlight some text and try again.")?;

    let (model, effective_skill_id) = {
        let cfg = lock(&state.config)?;
        let effective = skill_id.unwrap_or_else(|| cfg.default_skill_id.clone());
        (cfg.model.clone(), effective)
    };
    let skills_config = lock(&state.skills_config)?.clone();
    let client = state.http_client.clone();

    let system = crate::skills::build_system_prompt(&skills_config, Some(&effective_skill_id));
    let user_message = format!("<text>\n{text}\n</text>");

    let output = crate::rewrite::call_api_raw(&client, &system, &user_message, &model)
        .await
        .map_err(|e| e.to_string())?;

    log_history(&app, &state, &effective_skill_id, &text, &output);

    Ok(output)
}

// ── Config commands ───────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_config(state: State<AppState>) -> crate::config::Config {
    state.config.lock().unwrap().clone()
}

#[tauri::command]
pub fn save_config(
    config: crate::config::Config,
    state: State<AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let path = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("config.toml");

    crate::config::save(&config, &path).map_err(|e| e.to_string())?;
    *lock(&state.config)? = config;
    Ok(())
}

#[tauri::command]
pub fn open_settings(app: AppHandle) {
    crate::show_settings(&app);
}

#[tauri::command]
pub fn update_hotkey(
    hotkey: String,
    state: State<AppState>,
    app: AppHandle,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let old_hotkey = lock(&state.config)?.hotkey.clone();
    if hotkey == old_hotkey {
        return Ok(());
    }

    app.global_shortcut()
        .register(hotkey.as_str())
        .map_err(|_| format!("Hotkey '{hotkey}' is already in use by another app."))?;

    {
        let mut cfg = lock(&state.config)?;
        cfg.hotkey = hotkey;
        let path = app
            .path()
            .app_config_dir()
            .map_err(|e| e.to_string())?
            .join("config.toml");
        crate::config::save(&*cfg, &path).map_err(|e| e.to_string())?;
    }

    let _ = app.global_shortcut().unregister(old_hotkey.as_str());
    Ok(())
}

#[tauri::command]
pub fn update_super_hotkey(
    hotkey: String,
    state: State<AppState>,
    app: AppHandle,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let old_super = lock(&state.config)?.super_hotkey.clone();
    if hotkey == old_super {
        return Ok(());
    }

    app.global_shortcut()
        .register(hotkey.as_str())
        .map_err(|_| format!("Hotkey '{hotkey}' is already in use."))?;

    {
        let mut cfg = lock(&state.config)?;
        cfg.super_hotkey = hotkey;
        let path = app
            .path()
            .app_config_dir()
            .map_err(|e| e.to_string())?
            .join("config.toml");
        crate::config::save(&*cfg, &path).map_err(|e| e.to_string())?;
    }

    let _ = app.global_shortcut().unregister(old_super.as_str());
    Ok(())
}

#[tauri::command]
pub fn set_default_skill(
    skill_id: String,
    state: State<AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let mut cfg = lock(&state.config)?;
    cfg.default_skill_id = skill_id;
    let path = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("config.toml");
    crate::config::save(&*cfg, &path).map_err(|e| e.to_string())?;
    Ok(())
}

// ── Skills commands ───────────────────────────────────────────────────────────

fn skills_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|d| d.join("skills.json"))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_skills_config(state: State<AppState>) -> crate::skills::SkillsConfig {
    state.skills_config.lock().unwrap().clone()
}

#[tauri::command]
pub fn save_skills_config(
    config: crate::skills::SkillsConfig,
    state: State<AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let path = skills_path(&app)?;
    crate::skills::save(&config, &path).map_err(|e| e.to_string())?;
    *lock(&state.skills_config)? = config;
    Ok(())
}

#[tauri::command]
pub fn toggle_builtin_skill(
    id: String,
    enabled: bool,
    state: State<AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let mut config = lock(&state.skills_config)?.clone();
    config.builtin_enabled.insert(id, enabled);
    let path = skills_path(&app)?;
    crate::skills::save(&config, &path).map_err(|e| e.to_string())?;
    *lock(&state.skills_config)? = config;
    Ok(())
}

#[tauri::command]
pub fn create_skill(
    name: String,
    instructions: String,
    base_skill_id: Option<String>,
    state: State<AppState>,
    app: AppHandle,
) -> Result<crate::skills::Skill, String> {
    let mut config = lock(&state.skills_config)?.clone();
    let order = config.skills.len() as i32;
    let skill = crate::skills::Skill {
        id: crate::skills::new_id(),
        name,
        instructions,
        enabled: true,
        order,
        base_skill_id,
    };
    config.skills.push(skill.clone());
    let path = skills_path(&app)?;
    crate::skills::save(&config, &path).map_err(|e| e.to_string())?;
    *lock(&state.skills_config)? = config;
    Ok(skill)
}

#[tauri::command]
pub fn delete_skill(
    id: String,
    state: State<AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let mut config = lock(&state.skills_config)?.clone();
    config.skills.retain(|s| s.id != id);
    for (i, s) in config.skills.iter_mut().enumerate() {
        s.order = i as i32;
    }
    let path = skills_path(&app)?;
    crate::skills::save(&config, &path).map_err(|e| e.to_string())?;
    *lock(&state.skills_config)? = config;
    Ok(())
}

#[tauri::command]
pub fn reorder_skills(
    ids: Vec<String>,
    state: State<AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let mut config = lock(&state.skills_config)?.clone();
    let mut reordered: Vec<crate::skills::Skill> = Vec::with_capacity(ids.len());
    for (i, id) in ids.iter().enumerate() {
        if let Some(mut skill) = config.skills.iter().find(|s| s.id == *id).cloned() {
            skill.order = i as i32;
            reordered.push(skill);
        }
    }
    for skill in &config.skills {
        if !ids.contains(&skill.id) {
            let mut s = skill.clone();
            s.order = reordered.len() as i32;
            reordered.push(s);
        }
    }
    config.skills = reordered;
    let path = skills_path(&app)?;
    crate::skills::save(&config, &path).map_err(|e| e.to_string())?;
    *lock(&state.skills_config)? = config;
    Ok(())
}

// ── History commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_history(state: State<AppState>) -> Vec<crate::history::HistoryEntry> {
    let mut entries = state.history.lock().unwrap().entries.clone();
    entries.sort_by(|a, b| b.timestamp_ms.cmp(&a.timestamp_ms));
    entries
}
