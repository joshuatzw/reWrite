use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Listener, Manager, PhysicalPosition,
};

pub mod auth;
pub mod clipboard;
pub mod commands;
pub mod config;
pub mod foreground;
pub mod history;
pub mod rewrite;
pub mod secure_store;
pub mod skills;
#[cfg(target_os = "windows")]
pub mod esc_hook;

// ── Temporary diagnostics (overlay first-open hang) ─────────────────────────
// Timestamped, thread-tagged tracing to pinpoint where the main event loop
// stalls on the first couple of overlay opens. Remove once the hang is fixed.
use std::sync::OnceLock;
static TRACE_START: OnceLock<std::time::Instant> = OnceLock::new();

/// Latched true the moment the user genuinely opens the overlay. The startup
/// webview-warming pass shows the overlay off-screen and then hides it again —
/// once via an `overlay:ready` listener, once via a 5s fallback timer. On a cold
/// webview `overlay:ready` can arrive *seconds after* the first real open (12.6s
/// in one trace), so those hides would fire while the user is looking at the
/// overlay and yank it away — which is exactly the "overlay crashes on the first
/// opens after launch" symptom. Both warm-pass hides check this first and skip
/// once a real open has happened. See `show_overlay` and the warm block.
static OVERLAY_OPENED: AtomicBool = AtomicBool::new(false);
pub fn trace(where_: &str) {
    let t0 = TRACE_START.get_or_init(std::time::Instant::now);
    eprintln!(
        "[trace +{:>8.3}s tid={:?}] {}",
        t0.elapsed().as_secs_f64(),
        std::thread::current().id(),
        where_
    );
}

pub struct AppState {
    pub captured_text: Mutex<Option<String>>,
    pub original_clipboard: Mutex<Option<String>>,
    /// Output format chosen from the foreground app at capture time (HTML for
    /// rich-text targets like Outlook/Gmail, plain text otherwise). Sampled
    /// before any of our own windows steal foreground.
    pub foreground_format: Mutex<foreground::OutputFormat>,
    pub config: Mutex<config::Config>,
    pub skills_config: Mutex<skills::SkillsConfig>,
    pub history: Mutex<history::HistoryStore>,
    pub http_client: reqwest::Client,
    pub is_capturing: AtomicBool,
    /// In-flight guard covering the ENTIRE super-hotkey rewrite
    /// (capture → API → paste), so hammering the super-hotkey cannot fire
    /// overlapping API calls / racing clipboard writes. `is_capturing` only
    /// covers the capture phase; this covers the whole operation.
    pub is_rewriting: AtomicBool,
    pub auth_session: Mutex<Option<auth::AuthSession>>,
    pub subscription: Mutex<auth::SubscriptionCache>,
}

// ── Window helpers ────────────────────────────────────────────────────────────

pub fn show_overlay(app: &AppHandle) {
    // This is called from `on_hotkey`'s spawned async task — i.e. off the main
    // event-loop thread. Win32/WebView2 windows are thread-affine: show/focus/
    // move must run on the thread that owns the window (the main thread). Issuing
    // them from the tokio worker thread is undefined behaviour and crashed the
    // first couple of opens after launch, while the WebView2 controller was
    // still settling from the startup warm pass. Marshal onto the main thread —
    // the same rule `close_overlay` / `open_settings` already follow.
    trace("show_overlay: enter (pre run_on_main_thread)");
    // A real open supersedes the startup warm pass: from here on, the warm-pass
    // hides must not fire (they'd yank the overlay out from under the user).
    OVERLAY_OPENED.store(true, Ordering::SeqCst);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        trace("show_overlay: on main thread");
        if let Some(w) = handle.get_webview_window("overlay") {
            // Re-center in case the window is still parked off-screen from the
            // startup webview-warming pass (see the "Warm the overlay" block).
            trace("show_overlay: center start");
            let _ = w.center();
            trace("show_overlay: show start");
            let _ = w.show();
            trace("show_overlay: set_focus start");
            let _ = w.set_focus();
            trace("show_overlay: window ops done");
            #[cfg(target_os = "windows")]
            esc_hook::start(&handle);
            trace("show_overlay: esc_hook::start done");
            return;
        }
        let _ = tauri::WebviewWindowBuilder::new(&handle, "overlay", tauri::WebviewUrl::App("".into()))
            .title("")
            .decorations(false)
            .always_on_top(true)
            .transparent(true)
            .skip_taskbar(true)
            .inner_size(480.0, 430.0)
            .center()
            .focused(true)
            .build();
    });
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
        .inner_size(240.0, 240.0)
        .center()
        .focused(false)
        .build();
}

/// Switch the processing indicator to its "out of free rewrites" state — a red
/// glow — without rebuilding the window. The window is expected to already be
/// visible from a prior `show_processing` call.
pub fn show_processing_limit(app: &AppHandle) {
    let _ = app.emit("processing:limit", ());
    if let Some(w) = app.get_webview_window("processing") {
        let _ = w.show();
    }
}

pub fn hide_processing(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("processing") {
        let _ = w.hide();
    }
}

/// Whether a rewrite error corresponds to the user exhausting their free
/// rewrites / subscription limit (HTTP 402 codes surfaced by `call_api_raw`).
fn is_limit_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("limit") || m.contains("trial") || m.contains("quota") || m.contains("upgrade")
}

/// Return a currently-valid Supabase access token, refreshing it on demand when
/// the stored one has expired (or is within `is_expired`'s skew window).
///
/// Supabase access tokens live ~1 hour. We previously refreshed only at startup,
/// so a session left running past that hour sent an expired JWT on every
/// authenticated call and the Edge Function replied `401 Invalid JWT` until the
/// app was restarted. Routing every authenticated call through this helper keeps
/// the token fresh transparently. A refreshed session is persisted to disk and
/// state; a failed refresh clears the session so the UI can prompt a re-login.
///
/// Care: we snapshot the session and drop every lock/`State` guard before the
/// `.await`, then re-acquire state afterwards — never holding a lock across it.
pub async fn ensure_valid_token(app: &AppHandle) -> Option<String> {
    let (session, client) = {
        let state = app.try_state::<AppState>()?;
        let session = state.auth_session.lock().unwrap().as_ref().cloned()?;
        (session, state.http_client.clone())
    };

    if !auth::is_expired(&session) {
        return Some(session.access_token);
    }

    let auth_path = app.path().app_config_dir().ok().map(|d| d.join("auth.json"));

    match auth::refresh_session(&client, session).await {
        Ok(refreshed) => {
            if let Some(ref path) = auth_path {
                let _ = auth::save_session(&refreshed, path);
            }
            let token = refreshed.access_token.clone();
            if let Some(state) = app.try_state::<AppState>() {
                *state.auth_session.lock().unwrap() = Some(refreshed);
            }
            Some(token)
        }
        Err(_) => {
            if let Some(ref path) = auth_path {
                auth::clear_session(path);
            }
            if let Some(state) = app.try_state::<AppState>() {
                *state.auth_session.lock().unwrap() = None;
            }
            None
        }
    }
}

pub fn show_settings(app: &AppHandle) {
    // The Settings window is pre-warmed (hidden) at startup, so opening it is
    // just a show + focus — we never build a webview at runtime here. Building
    // a second webview from a command/menu callback on Windows can deadlock the
    // main event loop (the new window paints blank and the app freezes), so the
    // window must already exist. The build path below is only a safety net for
    // the unlikely case that pre-warming failed.
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    if let Ok(w) = tauri::WebviewWindowBuilder::new(app, "settings", tauri::WebviewUrl::App("".into()))
        .title("reWrite - Settings")
        .decorations(true)
        .always_on_top(false)
        .inner_size(1260.0, 870.0)
        .min_inner_size(900.0, 600.0)
        .center()
        .resizable(true)
        .build()
    {
        let _ = w.set_focus();
    }
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

    // Returning from the Stripe billing portal — the user may have changed
    // plan or cancelled, so re-sync subscription state.
    if url.starts_with("rewrite://portal-return") {
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
    trace("on_hotkey: enter");
    let Some(state) = app.try_state::<AppState>() else { return };

    // Sample the foreground app now, while the target still has focus — before
    // the overlay steals it — so we know whether to emit HTML or plain text.
    trace("on_hotkey: foreground::detect start");
    if let Ok(mut fmt) = state.foreground_format.lock() {
        *fmt = foreground::detect();
    }
    trace("on_hotkey: foreground::detect done");

    if state.is_capturing.swap(true, Ordering::SeqCst) {
        trace("on_hotkey: already capturing, bail");
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        trace("on_hotkey: capture_selection start");
        let result = tokio::task::spawn_blocking(clipboard::capture_selection).await;
        trace("on_hotkey: capture_selection done");

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

/// RAII guard that clears `AppState::is_rewriting` on drop, guaranteeing the
/// whole-rewrite in-flight flag is released on EVERY exit path of the
/// super-hotkey async task (early returns and the success path alike).
struct RewriteGuard {
    app: AppHandle,
}

impl Drop for RewriteGuard {
    fn drop(&mut self) {
        if let Some(state) = self.app.try_state::<AppState>() {
            state.is_rewriting.store(false, Ordering::SeqCst);
        }
    }
}

fn on_super_hotkey(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };

    // Whole-rewrite in-flight guard: a second concurrent super-hotkey press
    // while a rewrite is running is dropped silently. Set this BEFORE
    // `show_processing` so the second press shows nothing.
    if state.is_rewriting.swap(true, Ordering::SeqCst) {
        return;
    }

    if state.is_capturing.swap(true, Ordering::SeqCst) {
        // A capture (from either hotkey) is already in flight; release the
        // rewrite reservation we just took before bailing.
        state.is_rewriting.store(false, Ordering::SeqCst);
        return;
    }

    // Sample the foreground app now — after the guards (so a dropped duplicate
    // press can't overwrite the in-flight rewrite's format) but before
    // `show_processing` below steals focus, so the decision reflects the user's
    // real target app.
    if let Ok(mut fmt) = state.foreground_format.lock() {
        *fmt = foreground::detect();
    }

    show_processing(app);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Resets `is_rewriting` on drop — covers ALL returns below plus the
        // success path at the end of this async block.
        let _rewrite_guard = RewriteGuard { app: app.clone() };

        let capture_result = tokio::task::spawn_blocking(clipboard::capture_selection).await;

        let Some(state) = app.try_state::<AppState>() else {
            hide_processing(&app);
            return;
        };
        state.is_capturing.store(false, Ordering::SeqCst);

        // Require auth — refresh the token on demand if it has expired, so a
        // long-running session doesn't send a stale JWT and get a 401.
        let Some(access_token) = ensure_valid_token(&app).await else {
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

        let format = state
            .foreground_format
            .lock()
            .map(|f| *f)
            .unwrap_or_default();

        let (system, skill_name) = {
            let Ok(sc) = state.skills_config.lock() else {
                hide_processing(&app);
                return;
            };
            let system =
                skills::build_system_prompt(&sc, Some(&default_skill_id), format);
            let name = skills::skill_display_name(&sc, &default_skill_id);
            (system, name)
        };

        let client = state.http_client.clone();
        let user_message = format!("<text>\n{text}\n</text>");

        let result = match rewrite::call_api_raw(&client, &access_token, &system, &user_message, &model).await {
            Ok(o) => o,
            Err(e) => {
                if is_limit_error(&e.to_string()) {
                    // Show the red "out of free rewrites" glow briefly, then dismiss.
                    show_processing_limit(&app);
                    tokio::time::sleep(tokio::time::Duration::from_millis(2200)).await;
                }
                hide_processing(&app);
                return;
            }
        };
        let output = result.text;
        // For HTML targets the model returns markup; keep a plain-text form for
        // the clipboard fallback and for history / word-count.
        let plain_output = match format {
            foreground::OutputFormat::Html => clipboard::strip_html_tags(&output),
            foreground::OutputFormat::PlainText => output.clone(),
        };

        // Keep the local usage cache in step with the server-side count so the
        // Settings usage figure reflects super-hotkey rewrites too.
        if let Some(count) = result.rewrite_count {
            if let Ok(mut sub) = state.subscription.lock() {
                sub.rewrite_count = count;
            }
            let _ = app.emit("usage:updated", ());
        }

        {
            let entry = history::HistoryEntry {
                id: skills::new_id(),
                timestamp_ms: history::now_ms(),
                skill_id: default_skill_id,
                skill_name,
                input_text: text,
                output_text: plain_output.clone(),
                output_word_count: history::count_words(&plain_output),
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
        let _ = tokio::task::spawn_blocking(move || match format {
            foreground::OutputFormat::Html => clipboard::paste_html_and_restore(
                &output,
                &plain_output,
                &original,
                restore,
                restore_delay_ms,
            ),
            foreground::OutputFormat::PlainText => {
                clipboard::paste_and_restore(&output, &original, restore, restore_delay_ms)
            }
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
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
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
            foreground_format: Mutex::new(foreground::OutputFormat::default()),
            config: Mutex::new(config::Config::default()),
            skills_config: Mutex::new(skills::SkillsConfig::default()),
            history: Mutex::new(history::HistoryStore::default()),
            http_client: reqwest::Client::new(),
            is_capturing: AtomicBool::new(false),
            is_rewriting: AtomicBool::new(false),
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
                        if app_handle.try_state::<AppState>().is_none() { break }
                        if let Some(token) = ensure_valid_token(&app_handle).await {
                            let Some(state) = app_handle.try_state::<AppState>() else { break };
                            if let Ok(sub) =
                                auth::sync_subscription(&state.http_client, &token).await
                            {
                                *state.subscription.lock().unwrap() = sub;
                            }
                        }
                    }
                });
            }

            // ── Background update check ───────────────────────────────────────
            #[cfg(not(debug_assertions))]
            {
                use tauri_plugin_updater::UpdaterExt;
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let Ok(updater) = app_handle.updater() else { return };
                    let Ok(Some(update)) = updater.check().await else { return };
                    if update.download_and_install(|_, _| {}, || {}).await.is_ok() {
                        app_handle.request_restart();
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

                // ── Warm the overlay's webview ────────────────────────────
                // A window built hidden keeps its WebView2 content cold: the
                // engine defers loading the page until the window is first
                // shown. That made the very first overlay show race the cold
                // start — the native window was up (so Alt+F4 closed it) but
                // React and the Tauri IPC weren't live yet, so Esc and the X
                // button did nothing until a later show warmed it. Park the
                // window far off-screen and show it to force the webview to
                // load and mount React; React emits "overlay:ready", at which
                // point we hide it again. The next real show re-centers (see
                // show_overlay), so the off-screen parking stays invisible.
                let _ = overlay.set_position(PhysicalPosition::new(-32000, -32000));
                let warm_hide = overlay.clone();
                overlay.once("overlay:ready", move |_| {
                    if OVERLAY_OPENED.load(Ordering::SeqCst) {
                        trace("warm: overlay:ready but overlay already opened -> skip hide");
                        return;
                    }
                    trace("warm: overlay:ready received -> hide");
                    let _ = warm_hide.hide();
                    trace("warm: overlay:ready hide done");
                });
                // Safety net: if "overlay:ready" never arrives, don't leave the
                // window parked-and-shown off-screen forever.
                let warm_fallback = overlay.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    if OVERLAY_OPENED.load(Ordering::SeqCst) {
                        trace("warm: 5s fallback but overlay already opened -> skip hide");
                        return;
                    }
                    trace("warm: 5s fallback -> hide");
                    let _ = warm_fallback.hide();
                    trace("warm: 5s fallback hide done");
                });
                trace("warm: overlay.show() (off-screen) start");
                let _ = overlay.show();
                trace("warm: overlay.show() (off-screen) done");
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
            .inner_size(240.0, 240.0)
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

            // ── Pre-warm settings ─────────────────────────────────────────────
            // Build the (large, webview-heavy) Settings window once, hidden, so
            // that opening it later is a cheap show()/set_focus(). Building it on
            // demand from the overlay's `open_settings` command deadlocked the
            // main event loop on Windows, leaving both the overlay stuck on
            // screen and the Settings webview blank. Pre-warming sidesteps that
            // entirely and makes the window paint instantly when revealed.
            if let Ok(settings) = tauri::WebviewWindowBuilder::new(
                app.handle(),
                "settings",
                tauri::WebviewUrl::App("".into()),
            )
            .title("reWrite - Settings")
            .decorations(true)
            .always_on_top(false)
            .inner_size(1260.0, 870.0)
            .min_inner_size(900.0, 600.0)
            .center()
            .resizable(true)
            .visible(false)
            .build()
            {
                let settings_ref = settings.clone();
                settings.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        // Keep the window warm: hide instead of destroying it so
                        // it can be reopened instantly and never needs rebuilding.
                        api.prevent_close();
                        let _ = settings_ref.hide();
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
            commands::close_overlay,
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
