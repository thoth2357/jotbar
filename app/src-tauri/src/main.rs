// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;

use config::Config;
use jotbar_core::remote::{Remote, Target};
use jotbar_core::sync::{sync, SyncReport};
use jotbar_core::{now_ms, with_tags, NoteList, NoteMeta, Store};
use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, State, WebviewWindow, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

struct AppState {
    store: Mutex<Store>,
    config: Mutex<Config>,
    /// Connected sync target; the async mutex also makes syncs run one at a time.
    remote: tokio::sync::Mutex<Option<(Target, Remote)>>,
    status: Mutex<SyncStatus>,
    hotkey_error: Mutex<Option<String>>,
}

#[derive(Clone, Default, Serialize)]
struct SyncStatus {
    last_ok: Option<i64>,
    error: Option<String>,
    running: bool,
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------- window ----------

fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
}

fn show(app: &AppHandle) {
    if let Some(w) = main_window(app) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        let _ = w.emit("shown", ());
    }
}

fn toggle(app: &AppHandle) {
    if let Some(w) = main_window(app) {
        if w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) {
            let _ = w.hide();
        } else {
            show(app);
        }
    }
}

fn apply_size(app: &AppHandle, size: &str) {
    let (w, h) = config::size_px(size);
    if let Some(win) = main_window(app) {
        let _ = win.set_size(LogicalSize::new(w, h));
    }
}

fn register_hotkey(app: &AppHandle, hotkey: &str) {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let res = if hotkey.trim().is_empty() { Ok(()) } else { gs.register(hotkey.trim()) };
    *lock(&app.state::<AppState>().hotkey_error) = res.err().map(|e| e.to_string());
}

// ---------- notes ----------

/// The timeline: newest first, filtered by text and/or tags.
#[tauri::command]
fn list_notes(s: State<AppState>, query: Option<String>, tags: Option<Vec<String>>) -> NoteList {
    lock(&s.store).summaries(query.as_deref().unwrap_or(""), &tags.unwrap_or_default())
}

#[tauri::command]
fn read_note(s: State<AppState>, id: String) -> CmdResult<String> {
    lock(&s.store).read(&id).map_err(err)
}

/// Quick capture: text plus tags (stored as a trailing `#tag` line).
#[tauri::command]
fn create_note(s: State<AppState>, content: String, tags: Vec<String>) -> CmdResult<NoteMeta> {
    let content = with_tags(content.trim_end(), &tags);
    if content.trim().is_empty() {
        return Err("note is empty".into());
    }
    lock(&s.store).create(&content).map_err(err)
}

#[tauri::command]
fn save_note(s: State<AppState>, id: String, content: String) -> CmdResult<NoteMeta> {
    lock(&s.store).write(&id, &content).map_err(err)
}

#[tauri::command]
fn delete_note(s: State<AppState>, id: String) -> CmdResult<()> {
    lock(&s.store).delete(&id).map_err(err)
}

// ---------- config ----------

#[derive(Serialize)]
struct ConfigView {
    config: Config,
    notes_dir: String,
    hotkey_error: Option<String>,
}

#[tauri::command]
fn get_config(s: State<AppState>) -> ConfigView {
    ConfigView {
        config: lock(&s.config).clone(),
        notes_dir: lock(&s.store).dir().display().to_string(),
        hotkey_error: lock(&s.hotkey_error).clone(),
    }
}

#[tauri::command]
fn set_config(app: AppHandle, s: State<AppState>, config: Config) -> CmdResult<ConfigView> {
    let old = lock(&s.config).clone();
    if config.notes_dir != old.notes_dir {
        let store = Store::open(config.data_dir()).map_err(err)?;
        *lock(&s.store) = store;
        let _ = app.emit("notes-changed", ());
    }
    config.save().map_err(err)?;
    *lock(&s.config) = config.clone();
    if config.size != old.size {
        apply_size(&app, &config.size);
    }
    if config.hotkey != old.hotkey {
        register_hotkey(&app, &config.hotkey);
    }
    Ok(get_config(s))
}

#[tauri::command]
fn hide_window(app: AppHandle) {
    if let Some(w) = main_window(&app) {
        let _ = w.hide();
    }
}

/// pywal colours, if the user has them, so jotbar matches the desktop theme.
#[tauri::command]
fn theme() -> Option<serde_json::Value> {
    let path = dirs::cache_dir()?.join("wal/colors.json");
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

// ---------- sync ----------

async fn run_sync(app: &AppHandle) -> CmdResult<SyncReport> {
    let s = app.state::<AppState>();
    let target = {
        let c = lock(&s.config);
        match (&c.sync.target, c.sync.enabled) {
            (Some(t), true) => t.clone(),
            _ => return Err("sync is not set up".into()),
        }
    };
    let mut remote = s.remote.lock().await;
    lock(&s.status).running = true;
    let _ = app.emit("sync-status", lock(&s.status).clone());

    let result = async {
        if remote.as_ref().map(|(t, _)| t != &target).unwrap_or(true) {
            *remote = Some((target.clone(), Remote::connect(&target).map_err(err)?));
        }
        let (_, r) = remote.as_mut().expect("connected above");
        sync(&s.store, r).await.map_err(|e| format!("{e:#}"))
    }
    .await;

    {
        let mut st = lock(&s.status);
        st.running = false;
        match &result {
            Ok(_) => {
                st.last_ok = Some(now_ms());
                st.error = None;
            }
            Err(e) => st.error = Some(e.clone()),
        }
        let _ = app.emit("sync-status", st.clone());
    }
    if let Ok(rep) = &result {
        if rep.changed_local {
            let _ = app.emit("notes-changed", ());
        }
    }
    result
}

#[tauri::command]
async fn sync_now(app: AppHandle) -> CmdResult<SyncReport> {
    run_sync(&app).await
}

#[tauri::command]
fn sync_status(s: State<AppState>) -> SyncStatus {
    lock(&s.status).clone()
}

/// Check a target before saving it: connect and list.
#[tauri::command]
async fn test_target(target: Target) -> CmdResult<String> {
    let mut r = Remote::connect(&target).map_err(err)?;
    let n = r.list().await.map_err(|e| format!("{e:#}"))?;
    Ok(format!("Connected to {}: {} note(s) there", target.label(), n.len()))
}

// ---------- magic mode (local LLM completion via Ollama) ----------

#[tauri::command]
async fn complete(s: State<'_, AppState>, before: String, after: String) -> CmdResult<String> {
    let m = lock(&s.config).magic.clone();
    if !m.enabled || m.model.trim().is_empty() {
        return Ok(String::new());
    }
    let tail: String = {
        let chars: Vec<char> = before.chars().collect();
        chars[chars.len().saturating_sub(2000)..].iter().collect()
    };
    let head: String = after.chars().take(300).collect();
    let body = serde_json::json!({
        "model": m.model,
        "system": "You are an autocomplete engine inside a notes app. Continue the user's text \
                   from exactly where it stops. Reply with only the continuation: a few words \
                   up to one sentence. No quotes, no preamble, never repeat the given text.",
        "prompt": if head.trim().is_empty() { tail } else {
            format!("{tail}<CURSOR>{head}\n\n(Continue at <CURSOR>.)")
        },
        "stream": false,
        "keep_alive": "15m",
        "options": { "num_predict": 40, "temperature": 0.2, "stop": ["\n\n"] }
    });
    let url = format!("{}/api/generate", m.endpoint.trim_end_matches('/'));
    let res: serde_json::Value = reqwest::Client::new()
        .post(url)
        .timeout(Duration::from_secs(30))
        .json(&body)
        .send()
        .await
        .map_err(err)?
        .error_for_status()
        .map_err(err)?
        .json()
        .await
        .map_err(err)?;
    let text = res["response"].as_str().unwrap_or("").trim_end().to_string();
    Ok(text)
}

#[tauri::command]
async fn magic_models(endpoint: String) -> CmdResult<Vec<String>> {
    let url = format!("{}/api/tags", endpoint.trim_end_matches('/'));
    let res: serde_json::Value = reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("can't reach Ollama: {e}"))?
        .json()
        .await
        .map_err(err)?;
    Ok(res["models"]
        .as_array()
        .map(|a| a.iter().filter_map(|m| m["name"].as_str().map(String::from)).collect())
        .unwrap_or_default())
}

// ---------- app ----------

fn main() {
    let config = Config::load();
    let store = Store::open(config.data_dir()).expect("opening notes directory");
    let start_hidden = std::env::args().any(|a| a == "--hidden");

    tauri::Builder::default()
        // A second launch (e.g. `jotbar --toggle` bound to a compositor key)
        // is forwarded to the running instance.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if argv.iter().any(|a| a == "--toggle") {
                toggle(app);
            } else if argv.iter().any(|a| a == "--hide") {
                hide_window(app.clone());
            } else {
                show(app);
            }
        }))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        toggle(app);
                    }
                })
                .build(),
        )
        .manage(AppState {
            store: Mutex::new(store),
            config: Mutex::new(config.clone()),
            remote: tokio::sync::Mutex::new(None),
            status: Mutex::new(SyncStatus::default()),
            hotkey_error: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            list_notes, read_note, create_note, save_note, delete_note, get_config, set_config, hide_window, theme, sync_now,
            sync_status, test_target, complete, magic_models
        ])
        .on_window_event(|w, e| {
            if let WindowEvent::Focused(false) = e {
                let app = w.app_handle().clone();
                if !lock(&app.state::<AppState>().config).hide_on_blur {
                    return;
                }
                // Debounce: focus can flicker while the compositor maps the window.
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    if let Some(w) = main_window(&app) {
                        if !w.is_focused().unwrap_or(true) {
                            let _ = w.hide();
                        }
                    }
                });
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            apply_size(&handle, &config.size);
            register_hotkey(&handle, &config.hotkey);

            let toggle_i = MenuItem::with_id(app, "toggle", "Show / hide", true, None::<&str>)?;
            let sync_i = MenuItem::with_id(app, "sync", "Sync now", true, None::<&str>)?;
            let settings_i = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit jotbar", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(app, &[&toggle_i, &sync_i, &settings_i, &sep, &quit_i])?;
            TrayIconBuilder::with_id("main")
                .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?)
                .tooltip("jotbar")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "toggle" => toggle(app),
                    "sync" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = run_sync(&app).await;
                        });
                    }
                    "settings" => {
                        show(app);
                        let _ = app.emit("open-settings", ());
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, e| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = e
                    {
                        toggle(tray.app_handle());
                    }
                })
                .build(app)?;

            // Background sync loop.
            let bg = handle.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;
                loop {
                    let secs = lock(&bg.state::<AppState>().config).sync.interval_secs.max(10);
                    let _ = run_sync(&bg).await;
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                }
            });

            if !start_hidden {
                show(&handle);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running jotbar");
}
