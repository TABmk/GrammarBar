#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod codex;
mod edits;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, State,
};
use tokio::sync::{Mutex, Notify};

const DEFAULT_PROMPT: &str = "Correct my English grammar, spelling, and punctuation. Keep my meaning, tone, and wording whenever possible. Make only necessary changes. Explain each correction briefly in English.";

#[derive(Serialize, Deserialize)]
struct Settings {
    prompt: String,
    #[serde(default)]
    connected: bool,
    #[serde(default)]
    model: Option<String>,
}
struct AppState {
    server: Mutex<Option<codex::Server>>,
    cancel: Notify,
    directory: PathBuf,
    binary: PathBuf,
    hidden_at: std::sync::Mutex<Option<Instant>>,
}
impl AppState {
    async fn connect(&self, guard: &mut Option<codex::Server>) -> Result<(), String> {
        if guard.is_none() {
            *guard = Some(
                codex::Server::start(
                    &self.binary,
                    &self.directory.join("session"),
                    &self.directory.join("workspace"),
                )
                .await?,
            );
        }
        Ok(())
    }
    fn read_settings(&self) -> Result<Settings, String> {
        let path = self.directory.join("settings.json");
        if !path.exists() {
            return Ok(Settings {
                prompt: DEFAULT_PROMPT.into(),
                connected: false,
                model: None,
            });
        }
        let mut settings: Settings =
            serde_json::from_slice(&std::fs::read(path).map_err(|_| "Could not read settings.")?)
                .map_err(|_| "Could not read settings.")?;
        // Upgrade only the old built-in prompt; preserve all custom instructions.
        if settings.prompt == DEFAULT_PROMPT.replace("briefly in English.", "briefly in Russian.") {
            settings.prompt = DEFAULT_PROMPT.into();
        }
        Ok(settings)
    }
    fn write_settings(&self, settings: &Settings) -> Result<(), String> {
        let bytes = serde_json::to_vec(settings).map_err(|_| "Could not save settings.")?;
        let temporary = self.directory.join("settings.tmp");
        std::fs::write(&temporary, bytes)
            .and_then(|_| std::fs::rename(temporary, self.directory.join("settings.json")))
            .map_err(|_| "Could not save settings.".into())
    }
}
#[tauri::command]
fn settings(state: State<AppState>) -> Result<Settings, String> {
    state.read_settings()
}
#[tauri::command]
async fn save_settings(
    prompt: String,
    model: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if prompt.chars().count() > 10000 {
        return Err("Instructions are too long.".into());
    }
    if model
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.len() > 200)
    {
        return Err("Invalid model.".into());
    }
    let _guard = state.server.lock().await;
    let mut settings = state.read_settings()?;
    settings.prompt = prompt;
    settings.model = model;
    state.write_settings(&settings)
}
#[tauri::command]
async fn account(state: State<'_, AppState>) -> Result<Value, String> {
    let mut guard = state.server.lock().await;
    // No background launch on first use. A click on Connect starts authentication.
    if guard.is_none() && !state.read_settings()?.connected {
        return Ok(Value::Null);
    }
    state.connect(&mut guard).await?;
    match guard
        .as_mut()
        .unwrap()
        .request("account/read", json!({}))
        .await
    {
        Ok(value) => {
            let connected = value["account"]["type"] == "chatgpt";
            let mut settings = state.read_settings()?;
            settings.connected = connected;
            state.write_settings(&settings)?;
            Ok(if connected {
                value["account"].clone()
            } else {
                Value::Null
            })
        }
        Err(e) => {
            *guard = None;
            Err(e)
        }
    }
}
#[tauri::command]
async fn models(state: State<'_, AppState>) -> Result<Vec<Value>, String> {
    let mut guard = state.server.lock().await;
    state.connect(&mut guard).await?;
    let result =
        tokio::time::timeout(Duration::from_secs(60), guard.as_mut().unwrap().models()).await;
    match result {
        Ok(Ok(models)) => Ok(models),
        Ok(Err(error)) => {
            *guard = None;
            Err(error)
        }
        Err(_) => {
            *guard = None;
            Err("Could not load models. Please try again.".into())
        }
    }
}
#[tauri::command]
async fn login(state: State<'_, AppState>) -> Result<(), String> {
    let mut guard = state.server.lock().await;
    state.connect(&mut guard).await?;
    let result = guard
        .as_mut()
        .unwrap()
        .request("account/login/start", json!({"type":"chatgpt"}))
        .await;
    let value = match result {
        Ok(value) => value,
        Err(e) => {
            *guard = None;
            return Err(e);
        }
    };
    let url = value["authUrl"]
        .as_str()
        .ok_or("Could not start sign-in.")?;
    let parsed = tauri::Url::parse(url).map_err(|_| "Invalid sign-in URL.")?;
    if parsed.scheme() != "https"
        || !matches!(parsed.host_str(), Some("auth.openai.com" | "chatgpt.com"))
    {
        return Err("Invalid sign-in URL.".into());
    }
    let status = std::process::Command::new("/usr/bin/open")
        .arg(url)
        .status()
        .map_err(|_| "Could not open the browser.")?;
    if !status.success() {
        return Err("Could not open the browser.".into());
    }
    Ok(())
}
#[tauri::command]
async fn logout(state: State<'_, AppState>) -> Result<(), String> {
    let mut guard = state.server.lock().await;
    state.connect(&mut guard).await?;
    guard
        .as_mut()
        .unwrap()
        .request("account/logout", json!({}))
        .await?;
    *guard = None;
    let mut settings = state.read_settings()?;
    settings.connected = false;
    state.write_settings(&settings)
}
#[tauri::command]
async fn cancel_login(state: State<'_, AppState>) -> Result<(), String> {
    *state.server.lock().await = None;
    Ok(())
}
#[tauri::command]
async fn correct(text: String, state: State<'_, AppState>) -> Result<edits::Correction, String> {
    if text.trim().is_empty() {
        return Err("Paste some text.".into());
    }
    if text.chars().count() > 20000 {
        return Err("Maximum 20,000 characters.".into());
    }
    let mut guard = state
        .server
        .try_lock()
        .map_err(|_| "Wait for the current request to finish.")?;
    // Consume any cancellation left by a completed request.
    let _ = tokio::time::timeout(Duration::ZERO, state.cancel.notified()).await;
    state.connect(&mut guard).await?;
    let settings = state.read_settings()?;
    let workspace = state.directory.join("workspace");
    let result = tokio::select! {
        _ = state.cancel.notified() => Err("Cancelled".into()),
        result = tokio::time::timeout(Duration::from_secs(120), guard.as_mut().unwrap().correct(&text, &settings.prompt, settings.model.as_deref(), &workspace)) => {
            result.unwrap_or_else(|_| Err("The request timed out. Please try again.".into()))
        }
    };
    // Each correction uses a fresh process; no old turn can leak into the next request.
    *guard = None;
    result
}
#[tauri::command]
fn cancel(state: State<AppState>) {
    state.cancel.notify_one();
}
#[tauri::command]
fn hide(window: tauri::WebviewWindow) -> Result<(), String> {
    window.hide().map_err(|e| e.to_string())
}
#[tauri::command]
fn resize_height(window: tauri::WebviewWindow, height: f64) -> Result<(), String> {
    if !height.is_finite() {
        return Err("Invalid window height.".into());
    }
    let position = window.outer_position().map_err(|e| e.to_string())?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let monitor = window
        .current_monitor()
        .map_err(|e| e.to_string())?
        .ok_or("Display unavailable.")?;
    let available =
        (monitor.position().y as f64 + monitor.size().height as f64 - position.y as f64) / scale
            - 10.0;
    window
        .set_size(tauri::LogicalSize::new(
            700.0,
            height.clamp(380.0, available.max(380.0)),
        ))
        .map_err(|e| e.to_string())?;
    // Keep the top edge attached to the status item when changing content size.
    window.set_position(position).map_err(|e| e.to_string())
}
#[tauri::command]
fn copy(text: String) -> Result<(), String> {
    use std::io::Write;
    let mut child = std::process::Command::new("/usr/bin/pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|_| "Could not copy text.")?;
    child
        .stdin
        .take()
        .ok_or("Could not copy text.")?
        .write_all(text.as_bytes())
        .map_err(|_| "Could not copy text.")?;
    if !child.wait().map_err(|_| "Could not copy text.")?.success() {
        return Err("Could not copy text.".into());
    }
    Ok(())
}
// Use physical bounds throughout. On macOS monitor_from_point uses CoreGraphics
// points, while tray.rect returns physical pixels (different on Retina displays).
fn show(app: &tauri::AppHandle) {
    if let Err(error) = show_anchored(app) {
        eprintln!("Could not position popover: {error}");
    }
}
fn show_anchored(app: &tauri::AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("Window unavailable")?;
    let tray = app.tray_by_id("main-tray").ok_or("Tray unavailable")?;
    let rect = tray
        .rect()
        .map_err(|e| e.to_string())?
        .ok_or("Tray position unavailable")?;
    let point = rect.position.to_physical::<f64>(1.0);
    let monitors = window.available_monitors().map_err(|e| e.to_string())?;
    let monitor = monitors
        .iter()
        .find(|monitor| {
            physical_contains(
                point.x,
                point.y,
                monitor.position().x as f64,
                monitor.position().y as f64,
                monitor.size().width as f64,
                monitor.size().height as f64,
            )
        })
        .ok_or("Tray monitor unavailable")?;
    let scale = monitor.scale_factor();
    let size = rect.size.to_physical::<f64>(scale);
    let width = window.outer_size().map_err(|e| e.to_string())?.width as f64;
    let (x, arrow) = popover_position(
        point.x + size.width / 2.0,
        width,
        monitor.position().x as f64,
        monitor.size().width as f64,
        scale,
    );
    window
        .set_position(tauri::PhysicalPosition::new(x, point.y + size.height))
        .map_err(|e| e.to_string())?;
    window
        .eval(format!(
            "document.documentElement.style.setProperty('--anchor-x', '{arrow}px');"
        ))
        .map_err(|e| e.to_string())?;
    let _ = window.eval("document.body.classList.remove('opening'); void document.body.offsetWidth; document.body.classList.add('opening');");
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())
}
fn physical_contains(x: f64, y: f64, left: f64, top: f64, width: f64, height: f64) -> bool {
    x >= left && x < left + width && y >= top && y < top + height
}

fn popover_position(
    anchor: f64,
    width: f64,
    left: f64,
    screen_width: f64,
    scale: f64,
) -> (f64, f64) {
    let x = (anchor - width / 2.0).clamp(left, (left + screen_width - width).max(left));
    (x, (anchor - x) / scale)
}
#[cfg(test)]
mod popover_tests {
    use super::{physical_contains, popover_position};
    #[test]
    fn settings_preserve_model_and_read_older_files() {
        let old: super::Settings =
            serde_json::from_str(r#"{"prompt":"Keep my tone","connected":true}"#).unwrap();
        assert!(old.model.is_none());
        let chosen = super::Settings {
            model: Some("test-model".into()),
            ..old
        };
        let restored: super::Settings =
            serde_json::from_str(&serde_json::to_string(&chosen).unwrap()).unwrap();
        assert_eq!(restored.model.as_deref(), Some("test-model"));
        assert_eq!(restored.prompt, "Keep my tone");
    }
    #[test]
    fn retina_tray_coordinates_match_physical_monitor_bounds() {
        assert!(physical_contains(2600.0, 0.0, 0.0, 0.0, 2880.0, 1800.0));
        assert!(!physical_contains(2600.0, 0.0, 0.0, 0.0, 1440.0, 900.0));
        assert!(physical_contains(
            -100.0, 20.0, -1920.0, 0.0, 1920.0, 1080.0
        ));
        assert!(!physical_contains(0.0, 20.0, -1920.0, 0.0, 1920.0, 1080.0));
    }
    #[test]
    fn arrow_stays_under_icon_at_screen_edges_and_on_retina() {
        assert_eq!(
            popover_position(800.0, 700.0, 0.0, 1440.0, 1.0),
            (450.0, 350.0)
        );
        assert_eq!(
            popover_position(1380.0, 700.0, 0.0, 1440.0, 1.0),
            (740.0, 640.0)
        );
        assert_eq!(
            popover_position(-1200.0, 1400.0, -2880.0, 2880.0, 2.0),
            (-1900.0, 350.0)
        );
    }
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            settings,
            save_settings,
            models,
            account,
            login,
            logout,
            cancel_login,
            correct,
            cancel,
            hide,
            resize_height,
            copy
        ])
        .setup(|app| {
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let mut directory = app.path().app_data_dir()?;
            // Keep existing sessions at their original path (Codex credentials use it).
            let legacy = directory.with_file_name("local.textfixer");
            if legacy.is_dir() {
                directory = legacy;
            }
            std::fs::create_dir_all(directory.join("session"))?;
            std::fs::create_dir_all(directory.join("workspace"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
            }
            let binary = app
                .path()
                .resource_dir()?
                .join("resources/codex-runtime/bin/codex");
            app.manage(AppState {
                server: Mutex::new(None),
                cancel: Notify::new(),
                directory,
                binary,
                hidden_at: std::sync::Mutex::new(None),
            });
            let open = MenuItem::with_id(app, "open", "Open", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            TrayIconBuilder::with_id("main-tray")
                .icon(tauri::include_image!("icons/tray.png"))
                .icon_as_template(true)
                .tooltip("GrammarBar")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let just_hidden = app
                                .state::<AppState>()
                                .hidden_at
                                .lock()
                                .unwrap()
                                .is_some_and(|time| time.elapsed() < Duration::from_millis(250));
                            // Clicking the status item can send blur before mouse-up.
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else if !just_hidden {
                                show(app);
                            }
                        }
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::Focused(false) => {
                if window.is_visible().unwrap_or(false) {
                    *window.state::<AppState>().hidden_at.lock().unwrap() = Some(Instant::now());
                    let _ = window.hide();
                }
            }
            tauri::WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("Could not start GrammarBar")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app.try_state::<AppState>() {
                    if let Ok(mut server) = state.server.try_lock() {
                        *server = None;
                    }
                    state.cancel.notify_one();
                }
            }
        });
}
