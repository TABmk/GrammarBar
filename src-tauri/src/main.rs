#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod align;
mod analytics;
mod chunks;
mod claude;
mod cli;
mod codex;
mod edits;
#[cfg(target_os = "macos")]
mod macos;

use cli::{Provider, Tools};
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
    /// `None` in files written before providers existed: those used Codex.
    #[serde(default)]
    provider: Option<String>,
    /// "fast" (low reasoning effort, the default) or "thorough" (model default).
    #[serde(default)]
    speed: Option<String>,
    /// "system" (default), "light" or "dark".
    #[serde(default)]
    appearance: Option<String>,
    /// Mistake statistics; on unless turned off.
    #[serde(default)]
    analytics: Option<bool>,
}
impl Settings {
    fn provider(&self) -> Option<Provider> {
        match self.provider.as_deref() {
            Some(id) => Provider::parse(id).ok(),
            None if self.connected => Some(Provider::Codex),
            None => None,
        }
    }
    fn effort(&self) -> Option<&'static str> {
        (self.speed.as_deref() != Some("thorough")).then_some("low")
    }
    fn analytics(&self) -> bool {
        self.analytics.unwrap_or(true)
    }
    fn appearance(&self) -> &'static str {
        match self.appearance.as_deref() {
            Some("light") => "light",
            Some("dark") => "dark",
            _ => "system",
        }
    }
}
/// The window theme for an appearance setting; `None` follows the system.
fn theme(appearance: &str) -> Result<Option<tauri::Theme>, String> {
    match appearance {
        "system" => Ok(None),
        "light" => Ok(Some(tauri::Theme::Light)),
        "dark" => Ok(Some(tauri::Theme::Dark)),
        _ => Err("Invalid appearance.".into()),
    }
}
struct AppState {
    /// Running Codex app-server. Its lock also serializes corrections.
    server: Mutex<Option<codex::Server>>,
    claude_login: Mutex<Option<claude::Login>>,
    tools: Mutex<Option<Tools>>,
    cancel: Notify,
    directory: PathBuf,
    hidden_at: std::sync::Mutex<Option<Instant>>,
}
impl AppState {
    async fn tools(&self, refresh: bool) -> Tools {
        let mut cached = self.tools.lock().await;
        if refresh || cached.is_none() {
            *cached = Some(cli::detect().await);
        }
        cached.clone().unwrap_or_default()
    }
    fn analytics_path(&self) -> PathBuf {
        self.directory.join("analytics.jsonl")
    }
    fn workspace(&self) -> PathBuf {
        self.directory.join("workspace")
    }
    async fn connect(&self, guard: &mut Option<codex::Server>) -> Result<(), String> {
        if guard.is_none() {
            let tools = self.tools(false).await;
            *guard = Some(
                codex::Server::start(
                    &tools,
                    tools.require(Provider::Codex)?,
                    &self.directory.join("session"),
                    &self.workspace(),
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
                provider: None,
                speed: None,
                appearance: None,
                analytics: None,
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
    fn remember_connection(&self, provider: Provider, connected: bool) -> Result<(), String> {
        let mut settings = self.read_settings()?;
        if settings.provider() == Some(provider) || settings.provider.is_none() && connected {
            settings.provider = Some(provider.id().into());
            settings.connected = connected;
            self.write_settings(&settings)?;
        }
        Ok(())
    }
}
#[tauri::command]
fn settings(state: State<AppState>) -> Result<Value, String> {
    let settings = state.read_settings()?;
    Ok(json!({
        "prompt": settings.prompt,
        "model": settings.model,
        "connected": settings.connected,
        "provider": settings.provider().map(Provider::id),
        "speed": if settings.effort().is_some() { "fast" } else { "thorough" },
        "appearance": settings.appearance(),
        "analytics": settings.analytics(),
    }))
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
#[tauri::command]
fn set_analytics(enabled: bool, state: State<AppState>) -> Result<(), String> {
    let mut settings = state.read_settings()?;
    settings.analytics = Some(enabled);
    state.write_settings(&settings)
}
#[tauri::command]
fn analytics(days: Option<u64>, state: State<AppState>) -> Value {
    analytics::summary(&state.analytics_path(), days, now())
}
#[tauri::command]
fn clear_analytics(state: State<AppState>) -> Result<(), String> {
    analytics::clear(&state.analytics_path())
}
/// Applied and saved right away, like a native switch.
#[tauri::command]
fn set_appearance(
    appearance: String,
    window: tauri::WebviewWindow,
    state: State<AppState>,
) -> Result<(), String> {
    window
        .set_theme(theme(&appearance)?)
        .map_err(|e| e.to_string())?;
    let mut settings = state.read_settings()?;
    settings.appearance = Some(appearance);
    state.write_settings(&settings)
}
#[tauri::command]
fn launch_at_login(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}
#[tauri::command]
fn set_launch_at_login(enabled: bool, app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    if enabled {
        manager.enable()
    } else {
        manager.disable()
    }
    .map_err(|_| "Could not change the login item.".to_string())
}
#[tauri::command]
async fn save_settings(
    prompt: String,
    model: Option<String>,
    provider: String,
    speed: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if !matches!(speed.as_str(), "fast" | "thorough") {
        return Err("Invalid speed.".into());
    }
    if prompt.chars().count() > 10000 {
        return Err("Instructions are too long.".into());
    }
    if model
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.len() > 200)
    {
        return Err("Invalid model.".into());
    }
    let provider = Provider::parse(&provider)?;
    let _guard = state.server.lock().await;
    let mut settings = state.read_settings()?;
    if settings.provider() != Some(provider) {
        settings.connected = false;
    }
    settings.prompt = prompt;
    settings.model = model;
    settings.provider = Some(provider.id().into());
    settings.speed = Some(speed);
    state.write_settings(&settings)
}
/// Installed CLIs. `refresh` searches again, e.g. after the user installs one.
#[tauri::command]
async fn providers(refresh: bool, state: State<'_, AppState>) -> Result<Vec<Value>, String> {
    let tools = state.tools(refresh).await;
    Ok(Provider::ALL
        .iter()
        .map(|&provider| {
            json!({
                "id": provider.id(),
                "name": provider.name(),
                "installed": tools.get(provider).is_some(),
                "path": tools.get(provider),
                "installUrl": provider.install_url(),
            })
        })
        .collect())
}
#[tauri::command]
async fn account(provider: String, state: State<'_, AppState>) -> Result<Value, String> {
    let provider = Provider::parse(&provider)?;
    let account = match provider {
        Provider::Claude => {
            let tools = state.tools(false).await;
            claude::account(&tools, tools.require(Provider::Claude)?).await?
        }
        Provider::Codex => {
            let mut guard = state.server.lock().await;
            state.connect(&mut guard).await?;
            match guard
                .as_mut()
                .unwrap()
                .request("account/read", json!({}))
                .await
            {
                Ok(value) => {
                    (value["account"]["type"] == "chatgpt").then(|| value["account"].clone())
                }
                Err(e) => {
                    *guard = None;
                    return Err(e);
                }
            }
        }
    };
    if account.is_some() {
        *state.claude_login.lock().await = None;
    }
    state.remember_connection(provider, account.is_some())?;
    Ok(account.unwrap_or(Value::Null))
}
#[tauri::command]
async fn models(provider: String, state: State<'_, AppState>) -> Result<Vec<Value>, String> {
    match Provider::parse(&provider)? {
        Provider::Claude => {
            let tools = state.tools(false).await;
            claude::models(&tools, tools.require(Provider::Claude)?, &state.workspace()).await
        }
        Provider::Codex => {
            let mut guard = state.server.lock().await;
            state.connect(&mut guard).await?;
            let result =
                tokio::time::timeout(Duration::from_secs(60), guard.as_mut().unwrap().models())
                    .await;
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
    }
}
fn open_in_browser(url: &str) -> Result<(), String> {
    let status = std::process::Command::new("/usr/bin/open")
        .arg(url)
        .status()
        .map_err(|_| "Could not open the browser.")?;
    if !status.success() {
        return Err("Could not open the browser.".into());
    }
    Ok(())
}
/// Starts sign-in. Returns a Claude sign-in URL for the fallback link, if any.
#[tauri::command]
async fn login(provider: String, state: State<'_, AppState>) -> Result<Option<String>, String> {
    match Provider::parse(&provider)? {
        Provider::Claude => {
            let tools = state.tools(false).await;
            let (login, url) =
                claude::Login::start(&tools, tools.require(Provider::Claude)?).await?;
            *state.claude_login.lock().await = Some(login);
            Ok(url)
        }
        Provider::Codex => {
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
            open_in_browser(url)?;
            Ok(None)
        }
    }
}
/// Passes the code shown by claude.com after sign-in to the waiting Claude Code.
#[tauri::command]
async fn submit_login_code(code: String, state: State<'_, AppState>) -> Result<(), String> {
    state
        .claude_login
        .lock()
        .await
        .as_mut()
        .ok_or("Sign-in has ended. Start again.")?
        .submit_code(&code)
        .await
}
#[tauri::command]
fn open_sign_in(url: String) -> Result<(), String> {
    if !claude::is_sign_in_url(&url) {
        return Err("Invalid sign-in URL.".into());
    }
    open_in_browser(&url)
}
#[tauri::command]
fn open_install(provider: String) -> Result<(), String> {
    open_in_browser(Provider::parse(&provider)?.install_url())
}
#[tauri::command]
async fn logout(provider: String, state: State<'_, AppState>) -> Result<(), String> {
    let provider = Provider::parse(&provider)?;
    if provider == Provider::Codex {
        let mut guard = state.server.lock().await;
        state.connect(&mut guard).await?;
        guard
            .as_mut()
            .unwrap()
            .request("account/logout", json!({}))
            .await?;
        *guard = None;
    } else {
        let tools = state.tools(false).await;
        claude::logout(&tools, tools.require(Provider::Claude)?).await?;
    }
    state.remember_connection(provider, false)
}
#[tauri::command]
async fn cancel_login(state: State<'_, AppState>) -> Result<(), String> {
    *state.server.lock().await = None;
    *state.claude_login.lock().await = None;
    Ok(())
}
/// Parts corrected at the same time with Claude Code (one process each).
const PARALLEL_PARTS: usize = 3;
const PART_TIMEOUT: Duration = Duration::from_secs(120);

/// One attempt for one part: the draft, aligned with the part's source text.
async fn attempt(
    part: &str,
    draft: impl std::future::Future<Output = Result<edits::Draft, String>>,
) -> Result<Vec<edits::Segment>, String> {
    let draft = tokio::time::timeout(PART_TIMEOUT, draft)
        .await
        .map_err(|_| "The request timed out. Please try again.".to_string())??;
    align::align(part, &draft)
}
/// The first try uses the chosen effort; the retry uses the model default, in case
/// the model rejects that effort level.
fn efforts(effort: Option<&'static str>) -> [Option<&'static str>; 2] {
    [effort, None]
}
fn unchanged(part: &str) -> Vec<edits::Segment> {
    vec![edits::Segment {
        original: part.into(),
        replacement: part.into(),
        explanation: String::new(),
        ..Default::default()
    }]
}
type PartResult = Result<Vec<edits::Segment>, String>;
/// Corrects parts with Claude Code, several processes at a time.
struct ClaudeJob {
    tools: Tools,
    binary: PathBuf,
    workspace: PathBuf,
    instruction: String,
    model: Option<String>,
    effort: Option<&'static str>,
}
impl ClaudeJob {
    async fn run(
        &self,
        parts: &[String],
        mut on_done: impl FnMut(),
    ) -> Result<Vec<(usize, PartResult)>, String> {
        let limit = std::sync::Arc::new(tokio::sync::Semaphore::new(PARALLEL_PARTS));
        // Dropping the set (cancel) aborts every task and kills its process.
        let mut tasks = tokio::task::JoinSet::new();
        for (index, part) in parts.iter().cloned().enumerate() {
            let (tools, binary, workspace, limit) = (
                self.tools.clone(),
                self.binary.clone(),
                self.workspace.clone(),
                limit.clone(),
            );
            let (instruction, model, effort) =
                (self.instruction.clone(), self.model.clone(), self.effort);
            tasks.spawn(async move {
                let _permit = limit.acquire_owned().await;
                if part.trim().is_empty() {
                    return (index, Ok(unchanged(&part)));
                }
                let mut result = Err(String::new());
                for effort in efforts(effort) {
                    let request = edits::Request {
                        text: &part,
                        instruction: &instruction,
                        model: model.as_deref(),
                        effort,
                    };
                    result =
                        attempt(&part, claude::draft(&tools, &binary, &request, &workspace)).await;
                    if result.is_ok() {
                        break;
                    }
                }
                (index, result)
            });
        }
        let mut results = Vec::with_capacity(parts.len());
        while let Some(joined) = tasks.join_next().await {
            results.push(joined.map_err(|_| "Could not correct the text.".to_string())?);
            on_done();
        }
        Ok(results)
    }
}
#[tauri::command]
async fn correct(
    text: String,
    progress: tauri::ipc::Channel<Value>,
    state: State<'_, AppState>,
) -> Result<edits::Correction, String> {
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
    let settings = state.read_settings()?;
    let provider = settings
        .provider()
        .ok_or("Choose a provider in Settings.")?;
    let workspace = state.workspace();
    let tools = state.tools(false).await;
    let effort = settings.effort();
    // Sonnet is as accurate as Opus here and much faster, so it is Claude's default.
    let model = settings
        .model
        .clone()
        .or_else(|| (provider == Provider::Claude).then(|| "sonnet".to_owned()));
    let parts: Vec<String> = chunks::split(&text).into_iter().map(String::from).collect();
    let total = parts.len();
    let report = |done: usize| {
        let _ = progress.send(json!({"done": done, "total": total}));
    };
    report(0);
    if provider == Provider::Codex {
        state.connect(&mut guard).await?;
        guard.as_mut().unwrap().ensure_account().await?;
    }
    let job = async {
        let mut results: Vec<Option<Result<Vec<edits::Segment>, String>>> =
            (0..total).map(|_| None).collect();
        match provider {
            Provider::Claude => {
                let binary = tools.require(Provider::Claude)?.to_path_buf();
                let job = ClaudeJob {
                    tools: tools.clone(),
                    binary,
                    workspace: workspace.clone(),
                    instruction: settings.prompt.clone(),
                    model: model.clone(),
                    effort,
                };
                let mut done = 0;
                for (index, result) in job
                    .run(&parts, || {
                        done += 1;
                        report(done)
                    })
                    .await?
                {
                    results[index] = Some(result);
                }
            }
            Provider::Codex => {
                let server = guard.as_mut().unwrap();
                for (index, part) in parts.iter().enumerate() {
                    let mut result = Ok(unchanged(part));
                    if !part.trim().is_empty() {
                        for effort in efforts(effort) {
                            let request = edits::Request {
                                text: part,
                                instruction: &settings.prompt,
                                model: model.as_deref(),
                                effort,
                            };
                            result = attempt(part, server.draft(&request, &workspace)).await;
                            if result.is_ok() {
                                break;
                            }
                        }
                    }
                    results[index] = Some(result);
                    report(index + 1);
                }
            }
        }
        Ok::<_, String>(results)
    };
    let results = tokio::select! {
        _ = state.cancel.notified() => Err("Cancelled".into()),
        results = job => results,
    };
    // Each correction uses a fresh process; no old turn can leak into the next request.
    *guard = None;
    let mut correction = edits::Correction {
        segments: Vec::new(),
        failed: 0,
        parts: total,
    };
    let mut first_error = None;
    let mut checked: Vec<(&str, Vec<edits::Segment>)> = Vec::new();
    for (part, result) in parts.iter().zip(results?) {
        match result.unwrap_or_else(|| Err("Could not correct the text.".into())) {
            Ok(segments) => {
                if settings.analytics() {
                    checked.push((part, segments.clone()));
                }
                correction.segments.extend(segments)
            }
            Err(error) => {
                // Keep the part as written; the UI says it was not checked.
                correction.failed += 1;
                first_error.get_or_insert(error);
                correction.segments.extend(unchanged(part));
            }
        }
    }
    if correction.failed == total {
        return Err(first_error.unwrap_or_else(|| "Could not correct the text.".into()));
    }
    correction.validate(&text)?;
    let checked: Vec<(&str, &[edits::Segment])> =
        checked.iter().map(|(p, s)| (*p, s.as_slice())).collect();
    // Statistics are a side feature: a write error never fails the correction.
    if let Err(error) = analytics::record(&state.analytics_path(), &checked, now()) {
        eprintln!("{error}");
    }
    Ok(correction)
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
    // Apps opened from Finder get no LANG, and pbcopy then reads its input as
    // Mac Roman: a non-breaking space would paste as "¬†", quotes as "‚Äú".
    let mut child = std::process::Command::new("/usr/bin/pbcopy")
        .env("LC_ALL", "en_US.UTF-8")
        .env("LANG", "en_US.UTF-8")
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
    let x = popover_position(
        point.x + size.width / 2.0,
        width,
        monitor.position().x as f64,
        monitor.size().width as f64,
        scale,
    );
    // Like Control Center panels: a small gap below the menu bar, no arrow.
    window
        .set_position(tauri::PhysicalPosition::new(
            x,
            point.y + size.height + POPOVER_GAP * scale,
        ))
        .map_err(|e| e.to_string())?;
    let _ = window.eval("document.body.classList.remove('opening'); void document.body.offsetWidth; document.body.classList.add('opening');");
    window.show().map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    macos::round_window(&window);
    window.set_focus().map_err(|e| e.to_string())
}
fn physical_contains(x: f64, y: f64, left: f64, top: f64, width: f64, height: f64) -> bool {
    x >= left && x < left + width && y >= top && y < top + height
}

/// Logical points between the menu bar and the panel, and between the panel and
/// the screen edge.
const POPOVER_GAP: f64 = 6.0;
/// Centers the panel under the menu bar icon, keeping it inside the screen.
fn popover_position(anchor: f64, width: f64, left: f64, screen_width: f64, scale: f64) -> f64 {
    let margin = POPOVER_GAP * scale;
    (anchor - width / 2.0).clamp(
        left + margin,
        (left + screen_width - width - margin).max(left + margin),
    )
}
#[cfg(test)]
mod live_tests {
    /// Long text through the parallel Claude path:
    /// `cargo test -- --ignored live_claude_parallel --nocapture`
    #[test]
    #[ignore]
    fn live_claude_parallel_parts() {
        tauri::async_runtime::block_on(async {
            let tools = super::cli::detect().await;
            let binary = tools
                .require(super::Provider::Claude)
                .unwrap()
                .to_path_buf();
            let paragraph = "Yesterday me and my brother goes to the market, and we buyed some apple. It were very cheap and we was happy. ";
            let text = format!("{}\n\n", paragraph.repeat(6)).repeat(5);
            let parts: Vec<String> = super::chunks::split(&text)
                .into_iter()
                .map(String::from)
                .collect();
            let job = super::ClaudeJob {
                tools: tools.clone(),
                binary,
                workspace: std::env::temp_dir(),
                instruction: "Correct grammar. Make only necessary changes.".into(),
                model: Some("sonnet".into()),
                effort: Some("low"),
            };
            let started = std::time::Instant::now();
            let mut done = 0;
            let mut results = job.run(&parts, || done += 1).await.unwrap();
            results.sort_by_key(|(index, _)| *index);
            let segments: Vec<_> = results.into_iter().flat_map(|(_, r)| r.unwrap()).collect();
            let original: String = segments.iter().map(|s| s.original.as_str()).collect();
            assert_eq!(original, text);
            assert_eq!(done, parts.len());
            let changes = segments
                .iter()
                .filter(|s| s.original != s.replacement)
                .count();
            println!(
                "{} chars, {} parts, {changes} changes, {:?}",
                text.chars().count(),
                parts.len(),
                started.elapsed()
            );
            assert!(changes >= 30);
        });
    }
}

#[cfg(test)]
mod popover_tests {
    use super::{physical_contains, popover_position};
    #[test]
    fn appearance_defaults_to_system_and_maps_to_window_themes() {
        let old: super::Settings = serde_json::from_str(r#"{"prompt":"x"}"#).unwrap();
        assert_eq!(old.appearance(), "system");
        assert_eq!(super::theme("system"), Ok(None));
        assert_eq!(super::theme("light"), Ok(Some(tauri::Theme::Light)));
        assert_eq!(super::theme("dark"), Ok(Some(tauri::Theme::Dark)));
        assert!(super::theme("sepia").is_err());
        let odd: super::Settings =
            serde_json::from_str(r#"{"prompt":"x","appearance":"sepia"}"#).unwrap();
        assert_eq!(odd.appearance(), "system");
    }
    #[test]
    fn speed_defaults_to_fast() {
        let mut settings: super::Settings = serde_json::from_str(r#"{"prompt":"x"}"#).unwrap();
        assert_eq!(settings.effort(), Some("low"));
        settings.speed = Some("thorough".into());
        assert_eq!(settings.effort(), None);
    }
    #[test]
    fn settings_preserve_model_and_read_older_files() {
        let old: super::Settings =
            serde_json::from_str(r#"{"prompt":"Keep my tone","connected":true}"#).unwrap();
        assert!(old.model.is_none());
        assert_eq!(old.provider(), Some(super::Provider::Codex));
        let chosen = super::Settings {
            model: Some("test-model".into()),
            provider: Some("claude".into()),
            ..old
        };
        let restored: super::Settings =
            serde_json::from_str(&serde_json::to_string(&chosen).unwrap()).unwrap();
        assert_eq!(restored.model.as_deref(), Some("test-model"));
        assert_eq!(restored.prompt, "Keep my tone");
        assert_eq!(restored.provider(), Some(super::Provider::Claude));
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
    fn panel_centers_under_icon_and_stays_on_screen() {
        assert_eq!(popover_position(800.0, 700.0, 0.0, 1440.0, 1.0), 450.0);
        assert_eq!(popover_position(1380.0, 700.0, 0.0, 1440.0, 1.0), 734.0);
        assert_eq!(
            popover_position(-1200.0, 1400.0, -2880.0, 2880.0, 2.0),
            -1900.0
        );
        assert_eq!(
            popover_position(-2870.0, 1400.0, -2880.0, 2880.0, 2.0),
            -2868.0
        );
    }
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            settings,
            save_settings,
            providers,
            models,
            account,
            login,
            submit_login_code,
            open_sign_in,
            open_install,
            logout,
            cancel_login,
            correct,
            cancel,
            hide,
            resize_height,
            copy,
            set_appearance,
            launch_at_login,
            set_launch_at_login,
            set_analytics,
            analytics,
            clear_analytics
        ])
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
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
            if let Some(window) = app.get_webview_window("main") {
                let saved = std::fs::read(directory.join("settings.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok());
                if let Some(settings) = saved {
                    let _ = window.set_theme(theme(settings.appearance()).unwrap_or(None));
                }
            }
            app.manage(AppState {
                server: Mutex::new(None),
                claude_login: Mutex::new(None),
                tools: Mutex::new(None),
                cancel: Notify::new(),
                directory,
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
