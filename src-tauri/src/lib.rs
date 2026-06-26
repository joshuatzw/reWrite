use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    AppHandle, Manager,
};

pub mod clipboard;
pub mod commands;
pub mod config;
pub mod history;
pub mod rewrite;
pub mod skills;

pub struct AppState {
    pub captured_text: Mutex<Option<String>>,
    pub original_clipboard: Mutex<Option<String>>,
    pub config: Mutex<config::Config>,
    pub skills_config: Mutex<skills::SkillsConfig>,
    pub history: Mutex<history::HistoryStore>,
    pub http_client: reqwest::Client,
    pub is_capturing: AtomicBool,
}

// ── Window helpers ────────────────────────────────────────────────────────────

fn focus_existing(app: &AppHandle, label: &str) -> bool {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.show();
        let _ = w.set_focus();
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

pub fn show_settings(app: &AppHandle) {
    if focus_existing(app, "settings") {
        return;
    }
    let _ = tauri::WebviewWindowBuilder::new(app, "settings", tauri::WebviewUrl::App("".into()))
        .title("ReWrite — Settings")
        .decorations(true)
        .always_on_top(false)
        .inner_size(1260.0, 870.0)
        .min_inner_size(900.0, 600.0)
        .center()
        .resizable(true)
        .build();
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

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let capture_result = tokio::task::spawn_blocking(clipboard::capture_selection).await;

        let Some(state) = app.try_state::<AppState>() else { return };
        state.is_capturing.store(false, Ordering::SeqCst);

        let (text, original) = match capture_result {
            Ok(Ok((t, o))) if !t.is_empty() => (t, o),
            _ => return,
        };

        // Store captured text
        { *state.captured_text.lock().unwrap() = Some(text.clone()); }
        { *state.original_clipboard.lock().unwrap() = Some(original.clone()); }

        // Extract config data before awaiting
        let (model, default_skill_id, paste_delay_ms, restore, restore_delay_ms) = {
            let cfg = state.config.lock().unwrap().clone();
            (cfg.model, cfg.default_skill_id, cfg.paste_delay_ms, cfg.restore_clipboard, cfg.restore_delay_ms)
        };

        let (system, skill_name) = {
            let sc = state.skills_config.lock().unwrap();
            let system = skills::build_system_prompt(&sc, Some(&default_skill_id));
            let name = skills::skill_display_name(&sc, &default_skill_id);
            (system, name)
        };

        let client = state.http_client.clone();
        let user_message = format!("<text>\n{text}\n</text>");

        let output = match rewrite::call_api_raw(&client, &system, &user_message, &model).await {
            Ok(o) => o,
            Err(_) => return,
        };

        // Log to history
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

        // Paste result
        tokio::time::sleep(tokio::time::Duration::from_millis(paste_delay_ms)).await;
        let _ = tokio::task::spawn_blocking(move || {
            clipboard::paste_and_restore(&output, &original, restore, restore_delay_ms)
        })
        .await;
    });
}

// ── App entry point ───────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed { return; }

                    let Some(state) = app.try_state::<AppState>() else { return };
                    let super_hk = state.config.lock().unwrap().super_hotkey.clone();

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
        })
        .setup(|app| {
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

            let hotkey_ok = app.global_shortcut().register(hotkey.as_str()).is_ok();
            if !hotkey_ok {
                eprintln!("Failed to register hotkey '{hotkey}'");
            }

            // Register super hotkey only if different from main hotkey
            if super_hotkey != hotkey {
                if !app.global_shortcut().register(super_hotkey.as_str()).is_ok() {
                    eprintln!("Failed to register super hotkey '{super_hotkey}'");
                }
            }

            let settings_item = MenuItemBuilder::new("Settings").id("settings").build(app)?;
            let quit_item = MenuItemBuilder::new("Quit ReWrite").id("quit").build(app)?;
            let menu = MenuBuilder::new(app).items(&[&settings_item, &quit_item]).build()?;

            let tooltip = if hotkey_ok {
                format!("ReWrite  ·  {hotkey}")
            } else {
                format!("ReWrite  ·  ⚠ hotkey '{hotkey}' unavailable")
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
