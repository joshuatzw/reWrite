use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager,
};

pub mod auth;
pub mod clipboard;
pub mod commands;
pub mod config;
pub mod history;
pub mod rewrite;
pub mod skills;
#[cfg(target_os = "windows")]
pub mod esc_hook;

pub struct AppState {
    pub captured_text: Mutex<Option<String>>,
    pub original_clipboard: Mutex<Option<String>>,
    pub config: Mutex<config::Config>,
    pub skills_config: Mutex<skills::SkillsConfig>,
    pub history: Mutex<history::HistoryStore>,
    pub http_client: reqwest::Client,
    pub is_capturing: AtomicBool,
    pub auth_session: Mutex<Option<auth::AuthSession>>,
    pub subscription: Mutex<auth::SubscriptionCache>,
}

// ── Window helpers ────────────────────────────────────────────────────────────

fn focus_existing(app: &AppHandle, label: &str) -> bool {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.show();
        let _ = w.set_focus();
        #[cfg(target_os = "windows")]
        esc_hook::start(app);
        return true;
    }
    false
}

pub fn show_overlay(app: &AppHandle) {
    if focus_existing(app, "overlay") {
        return;
    }
    let _ = tauri::WebviewWindowBuilder::new(app, "overlay", tauri::WebviewUrl::App("".into()))
        .title("")
        .decorations(false)
        .always_on_top(true)
        .transparent(true)
        .skip_taskbar(true)
        .inner_size(480.0, 430.0)
        .center()
        .focused(true)
        .build();
}

pub fn show_processing(app: &AppHandle) {
    let _ = app.emit("processing:show", ());
    if let Some(w) = app.get_webview_window("processing") {
        let _ = w.show();
        return;
    }
    // Fallback if not pre-warmed
    let _ = tauri::WebviewWindowBuilder::new(app, "processing", tauri::WebviewUrl::App("".into()))
        .title("")
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .transparent(true)
        .skip_taskbar(true)
        .inner_size(160.0, 160.0)
        .center()
        .focused(false)
        .build();
}

pub fn hide_processing(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("processing") {
        let _ = w.hide();
    }
}

pub fn show_settings(app: &AppHandle) {
    if focus_existing(app, "settings") {
        return;
    }
    let _ = tauri::WebviewWindowBuilder::new(app, "settings", tauri::WebviewUrl::App("".into()))
        .title("reWrite — Settings")
        .decorations(true)
        .always_on_top(false)
        .inner_size(1260.0, 870.0)
        .min_inner_size(900.0, 600.0)
        .center()
        .resizable(true)
        .build();
}

// ── Deep-link handler ─────────────────────────────────────────────────────────

fn handle_deep_link(app: &AppHandle, url: &str) {
    if url.starts_with("rewrite://checkout-success") {
        show_settings(app);

        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let Some(state) = app.try_state::<AppState>() else { return };
            let access_token = state.auth_session.lock().unwrap().as_ref().map(|s| s.access_token.clone());
            let Some(access_token) = access_token else { return };

            if let Ok(sub) = auth::sync_subscription(&state.http_client, &access_token).await {
                *state.subscription.lock().unwrap() = sub;
            }

            let _ = app.emit("auth:complete", ());
        });
        return;
    }

    if url.starts_with("rewrite://checkout-cancelled") {
        show_settings(app);
        return;
    }

    if !url.starts_with("rewrite://auth") {
        return;
    }

    let Some((access_token, refresh_token, expires_at)) = auth::parse_auth_url(url) else {
        return;
    };

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(state) = app.try_state::<AppState>() else { return };
        let client = state.http_client.clone();

        let email = auth::get_user_email(&client, &access_token)
            .await
            .unwrap_or_default();

        let session = auth::AuthSession {
            access_token: access_token.clone(),
            refresh_token,
            expires_at,
            email,
        };

        if let Ok(path) = app.path().app_config_dir().map(|d| d.join("auth.json")) {
            let _ = auth::save_session(&session, &path);
        }

        *state.auth_session.lock().unwrap() = Some(session.clone());

        if let Ok(sub) = auth::sync_subscription(&client, &session.access_token).await {
            *state.subscription.lock().unwrap() = sub;
        }

        let _ = app.emit("auth:complete", ());
    });
}

// ── Hotkey handlers ───────────────────────────────────────────────────────────

fn on_hotkey(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    if state.is_capturing.swap(true, Ordering::SeqCst) {
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = tokio::task::spawn_blocking(clipboard::capture_selection).await;

        if let Some(s) = app.try_state::<AppState>() {
            s.is_capturing.store(false, Ordering::SeqCst);

            let (text_val, orig_val) = match result {
                Ok(Ok((text, original))) => (
                    (!text.is_empty()).then_some(text),
                    (!original.is_empty()).then_some(original),
                ),
                _ => (None, None),
            };

            if let Ok(mut g) = s.captured_text.lock() { *g = text_val; }
            if let Ok(mut g) = s.original_clipboard.lock() { *g = orig_val; }
        }

        show_overlay(&app);
    });
}

fn on_super_hotkey(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    if state.is_capturing.swap(true, Ordering::SeqCst) {
        return;
    }

    show_processing(app);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let capture_result = tokio::task::spawn_blocking(clipboard::capture_selection).await;

        let Some(state) = app.try_state::<AppState>() else {
            hide_processing(&app);
            return;
        };
        state.is_capturing.store(false, Ordering::SeqCst);

        // Require auth
        let access_token = {
            let guard = state.auth_session.lock().unwrap();
            guard.as_ref().map(|s| s.access_token.clone())
        };
        let Some(access_token) = access_token else {
            hide_processing(&app);
            return;
        };

        let (text, original) = match capture_result {
            Ok(Ok((t, o))) if !t.is_empty() => (t, o),
            _ => {
                hide_processing(&app);
                return;
            }
        };

        if let Ok(mut g) = state.captured_text.lock() { *g = Some(text.clone()); }
        if let Ok(mut g) = state.original_clipboard.lock() { *g = Some(original.clone()); }

        let (model, default_skill_id, paste_delay_ms, restore, restore_delay_ms) = {
            let Ok(cfg) = state.config.lock() else {
                hide_processing(&app);
                return;
            };
            (cfg.model.clone(), cfg.default_skill_id.clone(), cfg.paste_delay_ms, cfg.restore_clipboard, cfg.restore_delay_ms)
        };

        let (system, skill_name) = {
            let Ok(sc) = state.skills_config.lock() else {
                hide_processing(&app);
                return;
            };
            let system = skills::build_system_prompt(&sc, Some(&default_skill_id));
            let name = skills::skill_display_name(&sc, &default_skill_id);
            (system, name)
        };

        let client = state.http_client.clone();
        let user_message = format!("<text>\n{text}\n</text>");

        let output = match rewrite::call_api_raw(&client, &access_token, &system, &user_message, &model).await {
            Ok(o) => o,
            Err(_) => {
                hide_processing(&app);
                return;
            }
        };

        {
            let entry = history::HistoryEntry {
                id: skills::new_id(),
                timestamp_ms: history::now_ms(),
                skill_id: default_skill_id,
                skill_name,
                input_text: text,
                output_text: output.clone(),
                output_word_count: history::count_words(&output),
            };
            if let (Ok(mut h), Ok(path)) = (
                state.history.lock(),
                app.path().app_config_dir().map(|d| d.join("history.json")),
            ) {
                h.entries.push(entry);
                let _ = history::save(&h, &path);
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(paste_delay_ms)).await;
        let _ = tokio::task::spawn_blocking(move || {
            clipboard::paste_and_restore(&output, &original, restore, restore_delay_ms)
        })
        .await;

        hide_processing(&app);
    });
}

// ── App entry point ───────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_single_instance::init(|_app, _argv, _cwd| {}))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed { return; }

                    let Some(state) = app.try_state::<AppState>() else { return };
                    let Ok(cfg) = state.config.lock() else { return };
                    let super_hk = cfg.super_hotkey.clone();
                    drop(cfg);

                    let is_super = super_hk
                        .parse::<tauri_plugin_global_shortcut::Shortcut>()
                        .map(|sc| shortcut == &sc)
                        .unwrap_or(false);

                    if is_super {
                        on_super_hotkey(app);
                    } else {
                        on_hotkey(app);
                    }
                })
                .build(),
        )
        .manage(AppState {
            captured_text: Mutex::new(None),
            original_clipboard: Mutex::new(None),
            config: Mutex::new(config::Config::default()),
            skills_config: Mutex::new(skills::SkillsConfig::default()),
            history: Mutex::new(history::HistoryStore::default()),
            http_client: reqwest::Client::new(),
            is_capturing: AtomicBool::new(false),
            auth_session: Mutex::new(None),
            subscription: Mutex::new(auth::SubscriptionCache::default()),
        })
        .setup(|app| {
            // ── Load config, skills, history ──────────────────────────────────
            let config_path = app.path().app_config_dir()?.join("config.toml");
            let loaded_config = config::load(&config_path);
            let hotkey = loaded_config.hotkey.clone();
            let super_hotkey = loaded_config.super_hotkey.clone();
            *app.state::<AppState>().config.lock().unwrap() = loaded_config;

            let skills_path = app.path().app_config_dir()?.join("skills.json");
            let loaded_skills = skills::load(&skills_path);
            *app.state::<AppState>().skills_config.lock().unwrap() = loaded_skills;

            let history_path = app.path().app_config_dir()?.join("history.json");
            let loaded_history = history::load(&history_path);
            *app.state::<AppState>().history.lock().unwrap() = loaded_history;

            // ── Auth: load session, refresh + sync in background ──────────────
            let auth_path = app.path().app_config_dir()?.join("auth.json");
            let maybe_session = auth::load_session(&auth_path);

            if let Some(ref s) = maybe_session {
                *app.state::<AppState>().auth_session.lock().unwrap() = Some(s.clone());
            }

            if let Some(session) = maybe_session {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let Some(state) = app_handle.try_state::<AppState>() else { return };
                    let client = state.http_client.clone();
                    let auth_path = match app_handle.path().app_config_dir() {
                        Ok(d) => d.join("auth.json"),
                        Err(_) => return,
                    };

                    let session = if auth::is_expired(&session) {
                        match auth::refresh_session(&client, session).await {
                            Ok(refreshed) => {
                                let _ = auth::save_session(&refreshed, &auth_path);
                                *state.auth_session.lock().unwrap() = Some(refreshed.clone());
                                refreshed
                            }
                            Err(_) => {
                                auth::clear_session(&auth_path);
                                *state.auth_session.lock().unwrap() = None;
                                return;
                            }
                        }
                    } else {
                        session
                    };

                    if let Ok(sub) = auth::sync_subscription(&client, &session.access_token).await {
                        *state.subscription.lock().unwrap() = sub;
                    }
                });
            }

            // ── 24h subscription refresh timer ────────────────────────────────
            {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut interval =
                        tokio::time::interval(tokio::time::Duration::from_secs(24 * 3600));
                    interval.tick().await; // skip the immediate tick
                    loop {
                        interval.tick().await;
                        let Some(state) = app_handle.try_state::<AppState>() else { break };
                        let token = state
                            .auth_session
                            .lock()
                            .unwrap()
                            .as_ref()
                            .map(|s| s.access_token.clone());
                        if let Some(token) = token {
                            if let Ok(sub) =
                                auth::sync_subscription(&state.http_client, &token).await
                            {
                                *state.subscription.lock().unwrap() = sub;
                            }
                        }
                    }
                });
            }

            // ── Deep-link handler ─────────────────────────────────────────────
            {
                use tauri_plugin_deep_link::DeepLinkExt;

                // Register the scheme in the Windows registry during development
                #[cfg(debug_assertions)]
                app.deep_link().register_all()?;

                let app_handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        handle_deep_link(&app_handle, url.as_str());
                    }
                });

                // Handle URLs that launched the app (e.g. clicked link while app was closed)
                if let Ok(Some(urls)) = app.deep_link().get_current() {
                    for url in urls {
                        handle_deep_link(app.handle(), url.as_str());
                    }
                }
            }

            // ── Hotkeys ───────────────────────────────────────────────────────
            let hotkey_ok = app.global_shortcut().register(hotkey.as_str()).is_ok();
            if !hotkey_ok {
                eprintln!("Failed to register hotkey '{hotkey}'");
            }

            if super_hotkey != hotkey {
                if app.global_shortcut().register(super_hotkey.as_str()).is_err() {
                    eprintln!("Failed to register super hotkey '{super_hotkey}'");
                }
            }

            // ── Pre-warm overlay ──────────────────────────────────────────────
            if let Ok(overlay) = tauri::WebviewWindowBuilder::new(
                app.handle(),
                "overlay",
                tauri::WebviewUrl::App("".into()),
            )
            .title("")
            .decorations(false)
            .always_on_top(true)
            .transparent(true)
            .skip_taskbar(true)
            .inner_size(480.0, 430.0)
            .center()
            .focused(false)
            .visible(false)
            .build()
            {
                let overlay_ref = overlay.clone();
                overlay.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = overlay_ref.hide();
                        #[cfg(target_os = "windows")]
                        esc_hook::stop();
                    }
                });
            }

            // ── Pre-warm processing indicator ─────────────────────────────────
            if let Ok(proc_win) = tauri::WebviewWindowBuilder::new(
                app.handle(),
                "processing",
                tauri::WebviewUrl::App("".into()),
            )
            .title("")
            .decorations(false)
            .shadow(false)
            .always_on_top(true)
            .transparent(true)
            .skip_taskbar(true)
            .inner_size(160.0, 160.0)
            .center()
            .focused(false)
            .visible(false)
            .build()
            {
                let proc_ref = proc_win.clone();
                proc_win.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = proc_ref.hide();
                    }
                });
            }

            // ── Tray ──────────────────────────────────────────────────────────
            let settings_item = MenuItemBuilder::new("Settings").id("settings").build(app)?;
            let quit_item = MenuItemBuilder::new("Quit reWrite").id("quit").build(app)?;
            let menu = MenuBuilder::new(app).items(&[&settings_item, &quit_item]).build()?;

            let tooltip = if hotkey_ok {
                format!("reWrite  ·  {hotkey}")
            } else {
                format!("reWrite  ·  ⚠ hotkey '{hotkey}' unavailable")
            };

            TrayIconBuilder::new()
                .icon(tauri::include_image!("icons/32x32.png"))
                .menu(&menu)
                .tooltip(&tooltip)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "settings" => show_settings(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_captured_text,
            commands::rewrite_with_skill,
            commands::paste_text,
            commands::get_config,
            commands::save_config,
            commands::open_settings,
            commands::update_hotkey,
            commands::update_super_hotkey,
            commands::set_default_skill,
            commands::get_skills_config,
            commands::save_skills_config,
            commands::create_skill,
            commands::delete_skill,
            commands::reorder_skills,
            commands::toggle_builtin_skill,
            commands::get_history,
            commands::get_auth_state,
            commands::send_magic_link,
            commands::logout,
            commands::open_checkout,
            commands::open_billing_portal,
            commands::refresh_subscription,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
