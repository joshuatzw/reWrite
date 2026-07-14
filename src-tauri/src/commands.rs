use std::sync::{atomic::Ordering, Mutex, MutexGuard};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

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

struct PasteGuard<'a> {
    state: &'a AppState,
    trace_id: u64,
}

impl Drop for PasteGuard<'_> {
    fn drop(&mut self) {
        self.state.is_pasting.store(false, Ordering::SeqCst);
        crate::trace(&format!("paste#{}: paste guard released", self.trace_id));
    }
}

// ── Overlay commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_captured_text(state: State<AppState>) -> Option<String> {
    state.captured_text.lock().unwrap().clone()
}

#[tauri::command]
pub fn get_capture_error(state: State<AppState>) -> Option<String> {
    state.capture_error.lock().unwrap().clone()
}

#[tauri::command]
pub async fn paste_text(
    result: String,
    trace_id: Option<u64>,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let trace_id = trace_id.unwrap_or_else(crate::next_paste_trace_id);
    if state.is_pasting.swap(true, Ordering::SeqCst) {
        crate::trace(&format!(
            "paste#{trace_id}: dropped because another paste_text command is already running"
        ));
        return Ok(());
    }
    let _paste_guard = PasteGuard {
        state: &*state,
        trace_id,
    };

    let (original, paste_delay_ms, restore, restore_delay_ms, format) = {
        let original = lock(&state.original_clipboard)?.clone().unwrap_or_default();
        let format = *lock(&state.foreground_format)?;
        let cfg = lock(&state.config)?;
        (
            original,
            cfg.paste_delay_ms,
            cfg.restore_clipboard,
            cfg.restore_delay_ms,
            format,
        )
    };
    let caller = window.label().to_string();
    crate::trace(&format!(
        "paste#{trace_id}: command entered caller={caller} format={format:?} result={} original_len={} restore={} restore_delay_ms={} paste_delay_ms={}",
        crate::text_fingerprint(&result),
        original.len(),
        restore,
        restore_delay_ms,
        paste_delay_ms
    ));

    // bubble_menu is prewarmed and parked/reset instead of hidden, so a
    // successful paste must go through the same close path as dismissals.
    // Every other caller (Overlay) still gets a real hide.
    if caller == "bubble_menu" {
        crate::trace(&format!(
            "paste#{trace_id}: closing bubble_menu and restoring source focus"
        ));
        crate::hide_bubble_menu(window.app_handle());
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            crate::selection_watcher::focus_last_source_window();
        }
    } else {
        crate::trace(&format!(
            "paste#{trace_id}: hiding caller window and restoring paste target"
        ));
        window.hide().map_err(|e| e.to_string())?;
        crate::focus_paste_target_window(window.app_handle());
    }
    crate::trace(&format!(
        "paste#{trace_id}: sleeping before Ctrl+V for {paste_delay_ms}ms"
    ));
    tokio::time::sleep(tokio::time::Duration::from_millis(paste_delay_ms)).await;

    tokio::task::spawn_blocking(move || match format {
        crate::foreground::OutputFormat::Html => crate::clipboard::paste_html_and_restore(
            trace_id,
            &result,
            &crate::clipboard::strip_html_tags(&result),
            &original,
            restore,
            restore_delay_ms,
        ),
        crate::foreground::OutputFormat::PlainText => crate::clipboard::paste_and_restore(
            trace_id,
            &result,
            &original,
            restore,
            restore_delay_ms,
        ),
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

    let access_token = crate::ensure_valid_token(&app)
        .await
        .ok_or("not_logged_in")?;

    let (model, effective_skill_id) = {
        let cfg = lock(&state.config)?;
        let effective = skill_id.unwrap_or_else(|| cfg.default_skill_id.clone());
        (cfg.model.clone(), effective)
    };
    let skills_config = lock(&state.skills_config)?.clone();
    let format = *lock(&state.foreground_format)?;
    let client = state.http_client.clone();

    let system =
        crate::skills::build_system_prompt(&skills_config, Some(&effective_skill_id), format);
    let user_message = format!("<text>\n{text}\n</text>");

    let result =
        crate::rewrite::call_api_raw(&client, &access_token, &system, &user_message, &model)
            .await
            .map_err(|e| e.to_string())?;

    // History stores a plain-text rendering; HTML output is only for the paste.
    let logged = match format {
        crate::foreground::OutputFormat::Html => crate::clipboard::strip_html_tags(&result.text),
        crate::foreground::OutputFormat::PlainText => result.text.clone(),
    };
    log_history(&app, &state, &effective_skill_id, &text, &logged);

    // Keep the local usage cache in step with the server-side count so the
    // Settings "rewrites used this month" figure updates without a full re-sync.
    if let Some(count) = result.rewrite_count {
        if let Ok(mut sub) = state.subscription.lock() {
            sub.rewrite_count = count;
        }
        let _ = app.emit("usage:updated", ());
    }

    Ok(result.text)
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

    // Detect a bubble_enabled flip before overwriting stored state, so the
    // watcher can be started/stopped live — mirrors how `update_hotkey` /
    // `update_super_hotkey` re-register the global shortcut on change.
    let bubble_enabled_changed = {
        let prev = lock(&state.config)?;
        prev.bubble_enabled != config.bubble_enabled
    };
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let bubble_enabled = config.bubble_enabled;

    crate::config::save(&config, &path).map_err(|e| e.to_string())?;
    *lock(&state.config)? = config;

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    if bubble_enabled_changed {
        if bubble_enabled {
            crate::selection_watcher::start(&app);
        } else {
            crate::selection_watcher::stop();
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _ = bubble_enabled_changed;

    Ok(())
}

#[tauri::command]
pub fn close_overlay(app: AppHandle) {
    // Mirror the reliable dismissal used by `open_settings`: marshal the hide
    // onto the main event-loop thread. A `hide()` issued from JS (or the
    // low-level Esc hook thread) is silently ignored by Windows when the overlay
    // is the focused foreground window, so Esc / the X button appeared dead.
    // Running the hide here — the same thread `open_settings` uses — dismisses
    // the overlay regardless of focus. We also tear down the Esc hook, since it
    // should only be armed while the overlay is visible.
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(overlay) = handle.get_webview_window("overlay") {
            let _ = overlay.hide();
        }
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        crate::esc_hook::stop();
    });
}

// ── Selection bubble commands ─────────────────────────────────────────────────

// Takes no arguments from the frontend on purpose: Bubble.tsx used to pass the
// text/x/y it captured from its own `selection:detected` listener, but a live
// trace session showed that listener can still be empty by the time a (fast)
// human click arrives — event delivery into a webview that was hidden a
// moment earlier isn't instant, and the click can beat it, silently no-opping
// the whole bubble. Reading the anchor straight from `selection_watcher`'s own
// last-known state (the same Rust-side data the event was built from)
// sidesteps that race entirely.
#[cfg(any(target_os = "windows", target_os = "macos"))]
#[tauri::command]
pub fn bubble_clicked(state: State<AppState>, app: AppHandle) -> Result<(), String> {
    let anchor = crate::selection_watcher::last_anchor()
        .ok_or_else(|| "No active selection.".to_string())?;
    crate::trace(&format!(
        "bubble_clicked: enter, {} chars at ({}, {})",
        anchor.text.len(),
        anchor.x,
        anchor.y
    ));
    // Passive read only — no synthetic Ctrl+C. `anchor.text` already came from
    // the selection watcher's UIA probe, so we never need to re-copy from the
    // source window, which is what makes it safe to call this after focus may
    // have already shifted to our own bubble window.
    let snapshot = crate::clipboard::snapshot_clipboard().unwrap_or_default();
    *lock(&state.captured_text)? = Some(anchor.text);
    *lock(&state.original_clipboard)? = Some(snapshot);

    crate::hide_bubble(&app);
    crate::show_bubble_menu(&app, anchor.x, anchor.y);
    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
#[tauri::command]
pub fn bubble_clicked(_state: State<AppState>, _app: AppHandle) -> Result<(), String> {
    Err("Selection bubble is only supported on Windows and macOS.".to_string())
}

#[tauri::command]
pub fn close_bubble_menu(app: AppHandle) {
    crate::trace("close_bubble_menu: invoked from frontend");
    crate::hide_bubble_menu(&app);
}

/// Temporary diagnostic for the stuck-error-state investigation: lets
/// BubbleMenu.tsx surface a line in the same `crate::trace` terminal output
/// Rust-side events use, so we can see exactly which reset trigger (if any)
/// fires and when, relative to hide/show. Remove once resolved.
#[tauri::command]
pub fn debug_trace(msg: String) {
    crate::trace(&format!("frontend: {msg}"));
}

#[tauri::command]
pub fn open_settings(app: AppHandle) {
    // Marshal the whole sequence onto the main event-loop thread — every
    // window operation here must run there. We dismiss the overlay and tear
    // down its low-level Esc hook first (leaving the global keyboard hook armed
    // while another window takes focus is a needless liability), then reveal
    // the pre-warmed Settings window.
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(overlay) = handle.get_webview_window("overlay") {
            let _ = overlay.hide();
        }
        #[cfg(target_os = "windows")]
        crate::esc_hook::stop();
        crate::show_settings(&handle);
        // Land on the Settings tab (plan & billing) rather than Home — this
        // path is only reached from the overlay's "renew" link. The pre-warmed
        // window registers its listener at startup, so the event is live.
        let _ = handle.emit("settings:navigate", "settings");
    });
}

// ── Accessibility commands (macOS Phase 2 onboarding, see roadmap-mac.md) ─────

#[cfg(target_os = "macos")]
fn sync_selection_watcher_with_accessibility(app: &AppHandle, state: &AppState, granted: bool) {
    if granted {
        let bubble_enabled = state
            .config
            .lock()
            .map(|config| config.bubble_enabled)
            .unwrap_or(false);
        if bubble_enabled {
            crate::trace(
                "sync_selection_watcher_with_accessibility: permission granted, ensuring watcher",
            );
            crate::selection_watcher::start(app);
        }
    } else {
        crate::trace(
            "sync_selection_watcher_with_accessibility: permission missing, stopping watcher",
        );
        crate::selection_watcher::stop();
    }
}

/// Non-prompting query, safe to poll repeatedly (e.g. `AccessibilityView.tsx`'s
/// live status indicator). Never triggers the native permission dialog.
#[tauri::command]
pub fn check_accessibility_permission(app: AppHandle, state: State<AppState>) -> bool {
    #[cfg(target_os = "macos")]
    {
        let granted = crate::clipboard::accessibility_trusted(false);
        sync_selection_watcher_with_accessibility(&app, &state, granted);
        granted
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state);
        // Accessibility isn't a concept on other platforms — never block on it.
        true
    }
}

/// Triggers the native "reWrite would like to control this computer" system
/// prompt the first time it's called. macOS will not re-prompt if the user
/// already dismissed it once — that's expected OS behavior, not a bug; the
/// user has to grant it from System Settings after that (see
/// `open_accessibility_settings`).
#[tauri::command]
pub fn request_accessibility_permission(app: AppHandle, state: State<AppState>) -> bool {
    #[cfg(target_os = "macos")]
    {
        let granted = crate::clipboard::accessibility_trusted(true);
        sync_selection_watcher_with_accessibility(&app, &state, granted);
        granted
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state);
        true
    }
}

/// Opens System Settings straight to Privacy & Security → Accessibility,
/// same external-URL pattern as `open_checkout`/`open_billing_portal`.
#[tauri::command]
pub async fn open_accessibility_settings(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_opener::OpenerExt;

        // Best-known deep link for the Accessibility pane as of recent macOS
        // versions (Ventura/Sonoma/Sequoia). Unverified on a real device from
        // this environment — see project.md Known Gaps.
        app.opener()
            .open_url(
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                None::<&str>,
            )
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(())
    }
}

#[tauri::command]
pub fn update_hotkey(hotkey: String, state: State<AppState>, app: AppHandle) -> Result<(), String> {
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
pub fn delete_skill(id: String, state: State<AppState>, app: AppHandle) -> Result<(), String> {
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

// ── Auth / billing commands ───────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct AuthState {
    pub logged_in: bool,
    pub email: String,
    pub is_subscribed: bool,
    pub subscription_valid_until: Option<String>,
    pub rewrite_count: u32,
}

#[tauri::command]
pub fn get_auth_state(state: State<AppState>) -> AuthState {
    let session = state.auth_session.lock().unwrap();
    let sub = state.subscription.lock().unwrap();
    AuthState {
        logged_in: session.is_some(),
        email: session
            .as_ref()
            .map(|s| s.email.clone())
            .unwrap_or_default(),
        is_subscribed: sub.is_subscribed,
        subscription_valid_until: sub.subscription_valid_until.clone(),
        rewrite_count: sub.rewrite_count,
    }
}

#[tauri::command]
pub async fn send_magic_link(email: String, state: State<'_, AppState>) -> Result<(), String> {
    crate::auth::send_magic_link(&state.http_client, &email)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn logout(state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    if let Ok(path) = app.path().app_config_dir().map(|d| d.join("auth.json")) {
        crate::auth::clear_session(&path);
    }
    if let Ok(path) = app.path().app_config_dir().map(|d| d.join("subscription.json")) {
        crate::auth::clear_subscription(&path);
    }
    *state.auth_session.lock().unwrap() = None;
    *state.subscription.lock().unwrap() = crate::auth::SubscriptionCache::default();
    Ok(())
}

#[tauri::command]
pub async fn open_checkout(
    plan: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let access_token = crate::ensure_valid_token(&app)
        .await
        .ok_or("Not logged in")?;

    let url = crate::auth::create_checkout_url(&state.http_client, &access_token, &plan)
        .await
        .map_err(|e| e.to_string())?;

    app.opener()
        .open_url(&url, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_billing_portal(state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let access_token = crate::ensure_valid_token(&app)
        .await
        .ok_or("Not logged in")?;

    let url = crate::auth::create_portal_url(&state.http_client, &access_token)
        .await
        .map_err(|e| e.to_string())?;

    app.opener()
        .open_url(&url, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn refresh_subscription(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let access_token = crate::ensure_valid_token(&app)
        .await
        .ok_or("Not logged in")?;

    let sub = crate::auth::sync_subscription(&state.http_client, &access_token)
        .await
        .map_err(|e| e.to_string())?;

    crate::persist_subscription(&app, &sub);
    *state.subscription.lock().unwrap() = sub;
    Ok(())
}
