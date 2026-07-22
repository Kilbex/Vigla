// Step 5 — host wires the orchestrator into Tauri:
//   * builds a `Supervisor` at startup with a `Repository` opened on
//     the default DB path and a `WorkerEventSink` that forwards
//     canonical events to the frontend via `app.emit`,
//   * registers `start_mock_worker` / `stop_worker` Tauri commands,
//   * declares a `WorkerEvent` event type so the frontend gets the
//     full canonical event shape via tauri-specta bindings.

mod inbox_commands;
mod memory_commands;
mod mission_history_command;
mod playbook_store;
mod runtime_state;

use orchestrator::memory::MemoryRegistry;
use orchestrator::{
    cleanup_aborted_mission_artifacts as cleanup_aborted_mission_artifacts_service,
    continue_worker as continue_worker_service, get_worker_diff as get_worker_diff_service,
    parser::WorkerEventSink, start_claude_worker as start_claude_worker_service,
    start_codex_worker as start_codex_worker_service,
    start_gemini_worker as start_gemini_worker_service, MissionController, MissionEvent,
    MissionEventReceiver, MissionSpec, MissionWorkspace, Repository, ResolveAction, RetentionGuard,
    SpawnRequest, Supervisor, SupervisorError,
};
use playbook_store::{PlaybookStore, StoredPlaybook};
pub use runtime_state::{RuntimeHandle, RuntimeState};
use serde::{Deserialize, Serialize};
use specta::Type;
#[cfg(any(debug_assertions, test))]
use specta_typescript::{BigIntExportBehavior, Typescript};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_specta::{collect_commands, collect_events, Builder, Event};

#[derive(Serialize, Type)]
struct HealthDto {
    version: String,
    uptime_ms: u64,
}

/// Emitted to the frontend on each canonical worker event.
/// Transparent newtype so the wire payload is exactly the canonical
/// `event_schema::Event`.
#[derive(Serialize, Deserialize, Debug, Clone, Type, Event)]
#[serde(transparent)]
pub struct WorkerEvent(pub event_schema::Event);

/// Emitted to the frontend on each mission-level event. Transparent
/// newtype so the wire payload is exactly the
/// `orchestrator::MissionEvent` shape (mission_id, seq, ts, kind).
#[derive(Serialize, Deserialize, Debug, Clone, Type, Event)]
#[serde(transparent)]
pub struct MissionEventDto(pub MissionEvent);

#[tauri::command]
#[specta::specta]
fn health_check() -> HealthDto {
    let h = orchestrator::health_check();
    HealthDto {
        version: h.version.to_string(),
        uptime_ms: h.uptime_ms,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
enum StartupPhaseDto {
    Initializing,
    Ready,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
struct StartupStatusDto {
    phase: StartupPhaseDto,
    error: Option<String>,
}

/// P4 — Durable startup state. Polling observes both success and failure, so
/// neither result depends on attaching an event listener early enough.
#[tauri::command]
#[specta::specta]
fn startup_status(runtime: State<'_, RuntimeHandle>) -> StartupStatusDto {
    if runtime.is_ready() {
        StartupStatusDto {
            phase: StartupPhaseDto::Ready,
            error: None,
        }
    } else if let Some(error) = runtime.failure() {
        StartupStatusDto {
            phase: StartupPhaseDto::Failed,
            error: Some(error.to_owned()),
        }
    } else {
        StartupStatusDto {
            phase: StartupPhaseDto::Initializing,
            error: None,
        }
    }
}

#[tauri::command]
#[specta::specta]
async fn start_mock_worker(
    script: String,
    speed: Option<f64>,
    runtime: State<'_, RuntimeHandle>,
) -> Result<String, String> {
    let req = SpawnRequest {
        script,
        speed: speed.unwrap_or(1.0),
        task_title: "Demo task".into(),
    };
    runtime
        .ready()?
        .supervisor
        .spawn_mock(req)
        .await
        .map_err(|e: SupervisorError| e.to_string())
}

#[tauri::command]
#[specta::specta]
async fn stop_worker(worker_id: String, runtime: State<'_, RuntimeHandle>) -> Result<(), String> {
    runtime
        .ready()?
        .supervisor
        .stop(&worker_id)
        .await
        .map_err(|e: SupervisorError| e.to_string())
}

#[tauri::command]
#[specta::specta]
async fn start_claude_worker(
    prompt: String,
    cwd: String,
    max_turns: Option<u32>,
    runtime: State<'_, RuntimeHandle>,
) -> Result<String, String> {
    let rt = runtime.ready()?;
    start_claude_worker_service(&rt.supervisor, prompt, &cwd, max_turns)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
async fn start_codex_worker(
    prompt: String,
    cwd: String,
    runtime: State<'_, RuntimeHandle>,
) -> Result<String, String> {
    let rt = runtime.ready()?;
    start_codex_worker_service(&rt.supervisor, prompt, &cwd)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
async fn start_gemini_worker(
    prompt: String,
    cwd: String,
    runtime: State<'_, RuntimeHandle>,
) -> Result<String, String> {
    let rt = runtime.ready()?;
    start_gemini_worker_service(&rt.supervisor, prompt, &cwd)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
async fn list_recent_workers(
    limit: Option<u32>,
    runtime: State<'_, RuntimeHandle>,
) -> Result<Vec<event_schema::WorkerInfo>, String> {
    runtime
        .ready()?
        .repository
        .list_recent_workers(limit.unwrap_or(50))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
async fn get_worker_info(
    worker_id: String,
    runtime: State<'_, RuntimeHandle>,
) -> Result<event_schema::WorkerInfo, String> {
    runtime
        .ready()?
        .repository
        .get_worker_info_by_id(&worker_id)
        .await
        .map_err(|e| e.to_string())
}

#[derive(Serialize, Type)]
struct WorkerModelSwitchDto {
    worker_id: String,
    model: String,
    detail: String,
}

#[tauri::command]
#[specta::specta]
async fn switch_worker_model(
    worker_id: String,
    model: String,
    runtime: State<'_, RuntimeHandle>,
) -> Result<WorkerModelSwitchDto, String> {
    let model = normalize_model_name(&model)?;
    let rt = runtime.ready()?;
    let worker = rt
        .repository
        .get_worker_info_by_id(&worker_id)
        .await
        .map_err(|e| e.to_string())?;
    ensure_model_preference_supported(&worker)?;
    rt.repository
        .set_worker_model(&worker_id, Some(&model))
        .await
        .map_err(|e| e.to_string())?;
    Ok(WorkerModelSwitchDto {
        worker_id,
        detail: format!("Saved {model} for this worker's next continuation."),
        model,
    })
}

#[tauri::command]
#[specta::specta]
async fn replay_worker_events_page(
    worker_id: String,
    after_seq: Option<u64>,
    limit: u32,
    runtime: State<'_, RuntimeHandle>,
) -> Result<Vec<event_schema::Event>, String> {
    // Clamp to bound memory + IPC payload; sibling commands clamp too. A
    // renderer bug or bad actor could otherwise request a worker's entire
    // event log in one call (F-17).
    let limit = limit.min(1000);
    runtime
        .ready()?
        .repository
        .replay_for_worker_page(&worker_id, after_seq, limit)
        .await
        .map_err(|e| e.to_string())
}

#[derive(Serialize, Type)]
#[serde(rename_all = "snake_case")]
enum CliAuthState {
    Ready,
    MissingCli,
    NotLoggedIn,
    Unknown,
}

#[derive(Serialize, Type)]
struct CliAuthStatusDto {
    vendor: String,
    display_name: String,
    binary: String,
    binary_present: bool,
    state: CliAuthState,
    detail: String,
    login_command: String,
    docs_url: String,
}

#[derive(Clone, Copy, Debug)]
struct CliAuthSpec {
    vendor: &'static str,
    display_name: &'static str,
    binary: &'static str,
    login_command: &'static str,
    docs_url: &'static str,
}

const CLAUDE_AUTH_SPEC: CliAuthSpec = CliAuthSpec {
    vendor: "claude",
    display_name: "Claude CLI",
    binary: "claude",
    login_command: "claude auth login",
    docs_url: "https://code.claude.com/docs/en/authentication",
};

const CODEX_AUTH_SPEC: CliAuthSpec = CliAuthSpec {
    vendor: "codex",
    display_name: "Codex CLI",
    binary: "codex",
    login_command: "codex login",
    docs_url: "https://github.com/openai/codex",
};

const GEMINI_AUTH_SPEC: CliAuthSpec = CliAuthSpec {
    vendor: "gemini",
    display_name: "Gemini CLI (legacy)",
    binary: "gemini",
    login_command: "gemini",
    docs_url:
        "https://developers.google.com/gemini-code-assist/docs/deprecations/code-assist-individuals",
};

const ANTIGRAVITY_AUTH_SPEC: CliAuthSpec = CliAuthSpec {
    vendor: "antigravity",
    display_name: "Antigravity CLI",
    binary: "agy",
    login_command: "agy",
    docs_url: "https://antigravity.dev",
};

const KIRO_AUTH_SPEC: CliAuthSpec = CliAuthSpec {
    vendor: "kiro",
    display_name: "Kiro CLI",
    binary: "kiro-cli",
    login_command: "kiro-cli",
    docs_url: "https://kiro.dev",
};

const COPILOT_AUTH_SPEC: CliAuthSpec = CliAuthSpec {
    vendor: "copilot",
    display_name: "GitHub Copilot CLI",
    binary: "copilot",
    login_command: "copilot login",
    docs_url: "https://docs.github.com/en/copilot/github-copilot-in-the-cli",
};

#[derive(Serialize, Type)]
struct AppSettingsDto {
    version: String,
    db_path: String,
    configured_repo_root: Option<String>,
    mock_harness_path: String,
    mock_harness_present: bool,
    l1_quota_mock_enabled: bool,
    claude_present: bool,
    codex_present: bool,
    gemini_present: bool,
    antigravity_present: bool,
    kiro_present: bool,
    copilot_present: bool,
    cli_auth: Vec<CliAuthStatusDto>,
}

/// Optional developer default. Normal missions always use the folder selected
/// in the deploy panel; a GUI process cwd is not a trustworthy repository.
fn configured_repo_root() -> Option<std::path::PathBuf> {
    let raw = std::env::var("VIGLA_REPO_ROOT").ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    orchestrator::host_services::resolve_git_repo_root(&raw).ok()
}

/// The endurance heartbeat describes this host process, not any one project.
/// Root it beside the durable app database so launching from Finder (whose cwd
/// may be `/`) cannot write repository-looking state into an arbitrary folder.
fn endurance_storage_root() -> std::path::PathBuf {
    orchestrator::default_db_path()
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

#[tauri::command]
#[specta::specta]
async fn app_settings() -> AppSettingsDto {
    let db_path = orchestrator::default_db_path();
    let configured_repo_root =
        configured_repo_root().map(|path| path.to_string_lossy().into_owned());
    let mock_path = Supervisor::locate_mock_harness()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "<not found>".into());
    // Auth status probes include binary availability, so derive the
    // legacy "present on PATH" rows from the same pass.
    let cli_auth = check_cli_auth_statuses().await;
    let cli_present = |vendor: &str| {
        cli_auth
            .iter()
            .any(|status| status.vendor == vendor && status.binary_present)
    };
    AppSettingsDto {
        version: orchestrator::VERSION.to_string(),
        db_path: db_path.to_string_lossy().into_owned(),
        configured_repo_root,
        mock_harness_path: mock_path.clone(),
        mock_harness_present: mock_path != "<not found>",
        l1_quota_mock_enabled: std::env::var("VIGLA_L1_QUOTA_MOCK")
            .ok()
            .is_some_and(|v| v == "1"),
        claude_present: cli_present("claude"),
        codex_present: cli_present("codex"),
        gemini_present: cli_present("gemini"),
        antigravity_present: cli_present("antigravity"),
        kiro_present: cli_present("kiro"),
        copilot_present: cli_present("copilot"),
        cli_auth,
    }
}

#[tauri::command]
#[specta::specta]
async fn check_cli_auth() -> Vec<CliAuthStatusDto> {
    check_cli_auth_statuses().await
}

#[tauri::command]
#[specta::specta]
async fn open_cli_login(vendor: String) -> Result<(), String> {
    let spec = cli_auth_spec(&vendor)?;
    open_login_terminal(spec).await
}

#[tauri::command]
#[specta::specta]
async fn open_cli_auth_docs(vendor: String) -> Result<(), String> {
    let spec = cli_auth_spec(&vendor)?;
    open_external_url(spec.docs_url).await
}

fn cli_auth_spec(vendor: &str) -> Result<CliAuthSpec, String> {
    match vendor.trim().to_ascii_lowercase().as_str() {
        "claude" => Ok(CLAUDE_AUTH_SPEC),
        "codex" => Ok(CODEX_AUTH_SPEC),
        "gemini" => Ok(GEMINI_AUTH_SPEC),
        "antigravity" => Ok(ANTIGRAVITY_AUTH_SPEC),
        "kiro" => Ok(KIRO_AUTH_SPEC),
        "copilot" => Ok(COPILOT_AUTH_SPEC),
        other => Err(format!(
            "unknown CLI vendor `{other}`; expected claude, codex, gemini, antigravity, kiro, or copilot"
        )),
    }
}

async fn check_cli_auth_statuses() -> Vec<CliAuthStatusDto> {
    let (claude, codex, gemini, antigravity, kiro, copilot) = tokio::join!(
        check_cli_auth_status(CLAUDE_AUTH_SPEC),
        check_cli_auth_status(CODEX_AUTH_SPEC),
        check_cli_auth_status(GEMINI_AUTH_SPEC),
        check_cli_auth_status(ANTIGRAVITY_AUTH_SPEC),
        check_cli_auth_status(KIRO_AUTH_SPEC),
        check_cli_auth_status(COPILOT_AUTH_SPEC)
    );
    vec![claude, codex, gemini, antigravity, kiro, copilot]
}

async fn check_cli_auth_status(spec: CliAuthSpec) -> CliAuthStatusDto {
    let binary_present = probe_cli(spec.binary).await;
    if !binary_present {
        return cli_auth_status(
            spec,
            false,
            CliAuthState::MissingCli,
            format!("{} is not available on PATH.", spec.binary),
        );
    }

    match spec.vendor {
        "claude" => {
            let mut cmd = tokio::process::Command::new(spec.binary);
            cmd.args(["auth", "status", "--json"]);
            cmd.env("PATH", orchestrator::resolve_user_path());
            if probe_command_within(cmd, 3000).await {
                cli_auth_status(
                    spec,
                    true,
                    CliAuthState::Ready,
                    "Authenticated according to `claude auth status`.".into(),
                )
            } else if claude_auth_present() {
                cli_auth_status(
                    spec,
                    true,
                    CliAuthState::Ready,
                    "Claude credentials were detected locally.".into(),
                )
            } else {
                cli_auth_status(
                    spec,
                    true,
                    CliAuthState::NotLoggedIn,
                    "Run `claude auth login` to authenticate this CLI.".into(),
                )
            }
        }
        "codex" => {
            let mut cmd = tokio::process::Command::new(spec.binary);
            cmd.args(["login", "status"]);
            cmd.env("PATH", orchestrator::resolve_user_path());
            if probe_command_within(cmd, 3000).await {
                cli_auth_status(
                    spec,
                    true,
                    CliAuthState::Ready,
                    "Authenticated according to `codex login status`.".into(),
                )
            } else if codex_auth_present() {
                cli_auth_status(
                    spec,
                    true,
                    CliAuthState::Ready,
                    "Codex credentials were detected locally.".into(),
                )
            } else {
                cli_auth_status(
                    spec,
                    true,
                    CliAuthState::NotLoggedIn,
                    "Run `codex login` to authenticate this CLI.".into(),
                )
            }
        }
        "gemini" => {
            if gemini_auth_present() {
                cli_auth_status(
                    spec,
                    true,
                    CliAuthState::Ready,
                    "Gemini credentials were detected locally.".into(),
                )
            } else {
                cli_auth_status(
                    spec,
                    true,
                    CliAuthState::NotLoggedIn,
                    "Consumer Login with Google is no longer available. Configure supported enterprise credentials or an API key for this legacy CLI path.".into(),
                )
            }
        }
        _ => cli_auth_status(
            spec,
            true,
            CliAuthState::Unknown,
            "Vigla does not know how to check this CLI yet.".into(),
        ),
    }
}

fn cli_auth_status(
    spec: CliAuthSpec,
    binary_present: bool,
    state: CliAuthState,
    detail: String,
) -> CliAuthStatusDto {
    CliAuthStatusDto {
        vendor: spec.vendor.into(),
        display_name: spec.display_name.into(),
        binary: spec.binary.into(),
        binary_present,
        state,
        detail,
        login_command: spec.login_command.into(),
        docs_url: spec.docs_url.into(),
    }
}

fn claude_auth_present() -> bool {
    if env_var_nonempty("ANTHROPIC_API_KEY") || env_var_nonempty("CLAUDE_CODE_OAUTH_TOKEN") {
        return true;
    }
    home_config_file(".claude.json").is_some_and(|p| json_field_present(&p, &["oauthAccount"]))
}

fn codex_auth_present() -> bool {
    if env_var_nonempty("OPENAI_API_KEY") || env_var_nonempty("CODEX_API_KEY") {
        return true;
    }
    let base = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")));
    base.as_ref()
        .is_some_and(|p| json_field_present(&p.join("auth.json"), &["tokens", "OPENAI_API_KEY"]))
}

fn gemini_auth_present() -> bool {
    if env_var_nonempty("GEMINI_API_KEY") || env_var_nonempty("GOOGLE_API_KEY") {
        return true;
    }
    let adc_path = std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS").map(PathBuf::from);
    if adc_path.as_ref().is_some_and(|p| p.is_file()) {
        return true;
    }

    let base = std::env::var_os("GEMINI_CLI_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".gemini")));
    base.as_ref().is_some_and(|p| gemini_auth_files_present(p))
}

fn env_var_nonempty(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

fn home_config_file(name: &str) -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(name))
}

fn json_field_present(path: &Path, fields: &[&str]) -> bool {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => return false,
    };
    let json: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(json) => json,
        Err(_) => return false,
    };
    fields
        .iter()
        .any(|field| json_value_present(json.get(field)))
}

fn json_value_present(value: Option<&serde_json::Value>) -> bool {
    match value {
        Some(serde_json::Value::Null) | None => false,
        Some(serde_json::Value::Bool(value)) => *value,
        Some(serde_json::Value::Number(_)) => true,
        Some(serde_json::Value::String(value)) => !value.trim().is_empty(),
        Some(serde_json::Value::Array(value)) => !value.is_empty(),
        Some(serde_json::Value::Object(value)) => !value.is_empty(),
    }
}

fn gemini_auth_files_present(base: &Path) -> bool {
    ["oauth_creds.json", "tokens.json", "google_accounts.json"]
        .iter()
        .map(|name| base.join(name))
        .any(|p| {
            p.metadata()
                .map(|m| m.is_file() && m.len() > 0)
                .unwrap_or(false)
        })
}

async fn open_external_url(url: &str) -> Result<(), String> {
    let status = tokio::process::Command::new("/usr/bin/open")
        .arg(url)
        .status()
        .await
        .map_err(|e| format!("open docs: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open docs exited with status {status}"))
    }
}

async fn open_login_terminal(spec: CliAuthSpec) -> Result<(), String> {
    let path = shell_quote(orchestrator::resolve_user_path());
    let title = shell_quote(&format!("Vigla: {} login", spec.display_name));
    let command = spec.login_command;
    let docs = shell_quote(spec.docs_url);
    let script = format!(
        "export PATH={path}; clear; printf '%s\\n' {title}; printf 'Command: %s\\n' {cmd}; printf 'Docs: %s\\n\\n' {docs}; {command}; status=$?; printf '\\n'; if [ $status -eq 0 ]; then printf '%s\\n' 'Login command finished. Return to Vigla and refresh auth status.'; else printf 'Login command exited with status %s. Review the output above.\\n' \"$status\"; fi; printf '\\n'; read -r -p 'Press Return to close this terminal...' _; exit $status",
        cmd = shell_quote(command),
    );

    let status = tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(format!(
            "tell application \"Terminal\" to do script {}",
            applescript_string(&script)
        ))
        .arg("-e")
        .arg("tell application \"Terminal\" to activate")
        .status()
        .await
        .map_err(|e| format!("open login terminal: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open login terminal exited with status {status}"))
    }
}

fn normalize_model_name(model: &str) -> Result<String, String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("model must not be empty".into());
    }
    if model.len() > 128 {
        return Err("model must be 128 bytes or fewer".into());
    }
    if model
        .chars()
        .any(|c| c == '\n' || c == '\r' || c.is_control())
    {
        return Err("model must be a single-line CLI model name".into());
    }
    Ok(model.to_owned())
}

fn ensure_model_preference_supported(worker: &event_schema::WorkerInfo) -> Result<(), String> {
    if worker.vendor != event_schema::Vendor::Claude {
        return Err(
            "model selection for a continuation is currently available only for Claude workers"
                .into(),
        );
    }
    let binary = worker.cli_binary.trim();
    let basename = Path::new(binary)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(binary);
    if basename == "claude" {
        Ok(())
    } else {
        Err(format!(
            "worker {} was not launched by the real claude CLI",
            worker.id
        ))
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn applescript_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

// ── Step 22 — playbook persistence IPC ─────────────────────────────

#[tauri::command]
#[specta::specta]
async fn list_playbooks(runtime: State<'_, RuntimeHandle>) -> Result<Vec<StoredPlaybook>, String> {
    runtime
        .ready()?
        .playbook_store
        .list()
        .map_err(|e| format!("list playbooks: {e}"))
}

#[tauri::command]
#[specta::specta]
async fn save_playbook(
    id: String,
    json: String,
    runtime: State<'_, RuntimeHandle>,
) -> Result<(), String> {
    runtime.ready()?.playbook_store.save(&id, &json)
}

#[tauri::command]
#[specta::specta]
async fn delete_playbook(id: String, runtime: State<'_, RuntimeHandle>) -> Result<(), String> {
    runtime.ready()?.playbook_store.delete(&id)
}

fn validate_mind_map_file_path(path: &str) -> Result<std::path::PathBuf, String> {
    let p = std::path::PathBuf::from(path);
    if !p.is_absolute() {
        return Err("path must be absolute".into());
    }
    if p.components().any(|c| c == std::path::Component::ParentDir) {
        return Err("path must not contain '..' components".into());
    }
    if !p
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
    {
        return Err("path must end in .svg".into());
    }
    match p.file_name().and_then(|s| s.to_str()) {
        Some(name) if !name.starts_with('.') => Ok(p),
        Some(_) => Err("filename must not start with '.'".into()),
        None => Err("path has no filename component".into()),
    }
}

fn validate_mind_map_filename(filename: &str) -> Result<&str, String> {
    let path = Path::new(filename);
    if path.components().count() != 1 {
        return Err("filename must not contain a path".into());
    }
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
    {
        return Err("filename must end in .svg".into());
    }
    match path.file_name().and_then(|name| name.to_str()) {
        Some(name) if !name.starts_with('.') => Ok(name),
        Some(_) => Err("filename must not start with '.'".into()),
        None => Err("filename is missing".into()),
    }
}

const MAX_MIND_MAP_EXPORT_BYTES: usize = 8 * 1024 * 1024;

fn validate_mind_map_contents(contents: &str) -> Result<(), String> {
    if contents.len() > MAX_MIND_MAP_EXPORT_BYTES {
        return Err(format!(
            "SVG exceeds the {MAX_MIND_MAP_EXPORT_BYTES}-byte export limit"
        ));
    }

    let mut document = contents.trim_start();
    if let Some(after_prefix) = document.strip_prefix("<?xml") {
        let Some(declaration_end) = after_prefix.find("?>") else {
            return Err("contents must be an SVG document".into());
        };
        document = after_prefix[declaration_end + 2..].trim_start();
    }

    let Some(after_root_name) = document.strip_prefix("<svg") else {
        return Err("contents must be an SVG document".into());
    };
    if !after_root_name
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_whitespace() || matches!(character, '/' | '>'))
    {
        return Err("contents must be an SVG document".into());
    }

    Ok(())
}

fn write_mind_map_to_path(path: &Path, contents: &str) -> Result<(), String> {
    let validated = validate_mind_map_file_path(&path.to_string_lossy())?;
    validate_mind_map_contents(contents)?;
    std::fs::write(validated, contents).map_err(|e| format!("write failed: {e}"))
}

/// Ask the native host to choose the destination and then write the generated
/// SVG. The renderer supplies only a suggested filename, never an authorized
/// filesystem path.
#[tauri::command]
#[specta::specta]
async fn save_mind_map_file(
    filename: String,
    contents: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let filename = validate_mind_map_filename(&filename)?.to_owned();
    validate_mind_map_contents(&contents)?;

    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("SVG image", &["svg"])
        .set_file_name(filename)
        .save_file(move |path| {
            let _ = sender.send(path);
        });

    let Some(path) = receiver
        .await
        .map_err(|_| "save dialog closed unexpectedly".to_string())?
    else {
        return Ok(());
    };
    let path = path
        .into_path()
        .map_err(|e| format!("selected path is not a local file: {e}"))?;
    write_mind_map_to_path(&path, &contents)
}

/// Step 25 — continue a worker with a follow-up prompt. Resumes the
/// worker's CLI session if supported, or returns an error if resume is
/// not available or the worker is still running.
#[tauri::command]
#[specta::specta]
async fn continue_worker(
    worker_id: String,
    prompt: String,
    runtime: State<'_, RuntimeHandle>,
) -> Result<(), String> {
    let rt = runtime.ready()?;
    continue_worker_service(&rt.supervisor, &worker_id, &prompt)
        .await
        .map_err(|e| e.to_string())
}

/// Step 25 — retry a worker with its last prompt. Re-runs the original
/// prompt against the worker's resumed CLI session.
#[tauri::command]
#[specta::specta]
async fn retry_worker(worker_id: String, runtime: State<'_, RuntimeHandle>) -> Result<(), String> {
    runtime
        .ready()?
        .supervisor
        .retry_worker(&worker_id)
        .await
        .map_err(|e: SupervisorError| e.to_string())
}

/// Batch 2 — get unified diff for a worker's worktree.
/// Runs `git diff --unified=3` in the worker's cwd and returns the output.
/// Returns empty string if git is not available or the cwd is not a git repo.
#[tauri::command]
#[specta::specta]
async fn get_worker_diff(
    worker_id: String,
    runtime: State<'_, RuntimeHandle>,
) -> Result<String, String> {
    let rt = runtime.ready()?;
    get_worker_diff_service(&rt.supervisor, &worker_id)
        .await
        .map_err(|e| e.to_string())
}

/// Start a mission. Business routing and lifecycle locking live in
/// `orchestrator::MissionController`; this command only forwards the
/// resulting event stream to Tauri.
#[tauri::command]
#[specta::specta]
async fn start_mission(
    spec: MissionSpec,
    cwd: String,
    runtime: State<'_, RuntimeHandle>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let started = runtime
        .ready()?
        .mission_controller
        .start_mission(spec, &cwd)
        .await
        .map_err(|e| e.to_string())?;
    forward_mission_events(started.events, app);
    Ok(started.mission_id)
}

/// Abort the active mission. Refuses if no mission is active or if
/// it has already terminated.
#[tauri::command]
#[specta::specta]
async fn abort_mission(runtime: State<'_, RuntimeHandle>) -> Result<(), String> {
    runtime
        .ready()?
        .mission_controller
        .abort_mission()
        .await
        .map_err(|e| e.to_string())
}

/// Apply the user's executable final disposition (Merge / Discard).
/// The reserved Extend wire variant fails closed in the runtime.
/// Blocks until the active mission reaches `CompletePendingMerge`.
#[tauri::command]
#[specta::specta]
async fn resolve_mission(
    action: ResolveAction,
    runtime: State<'_, RuntimeHandle>,
) -> Result<(), String> {
    runtime
        .ready()?
        .mission_controller
        .resolve_mission(action)
        .await
        .map_err(|e| e.to_string())
}

/// QC-2: confirm the supervisor's currently-proposed plan. Only
/// valid when the active mission is paused at
/// `PendingPlanApproval` (i.e. it was started with
/// `MissionSpec.confirm_plan == true`). Follows the same
/// lock-discipline as `resolve_mission` so a slow inner await
/// doesn't queue every other IPC call.
#[tauri::command]
#[specta::specta]
async fn confirm_plan(generation: u32, runtime: State<'_, RuntimeHandle>) -> Result<(), String> {
    runtime
        .ready()?
        .mission_controller
        .confirm_plan(generation)
        .await
        .map_err(|e| e.to_string())
}

/// QC-2: request a regenerated plan with optional natural-language
/// feedback. Only valid when the active mission is paused at
/// `PendingPlanApproval`.
#[tauri::command]
#[specta::specta]
async fn regenerate_plan(
    generation: u32,
    hint: Option<String>,
    runtime: State<'_, RuntimeHandle>,
) -> Result<(), String> {
    runtime
        .ready()?
        .mission_controller
        .regenerate_plan(generation, hint)
        .await
        .map_err(|e| e.to_string())
}

/// QC-3: reject the proposed plan and abort the mission. Only valid
/// while the active mission is paused at `PendingPlanApproval`.
/// Mirrors the shape of `confirm_plan` / `regenerate_plan`; the
/// orchestrator emits `PlanRejected { generation, reason }` followed
/// by `Aborted { reason }` (with the user's reason embedded).
#[tauri::command]
#[specta::specta]
async fn reject_plan(
    generation: u32,
    reason: Option<String>,
    runtime: State<'_, RuntimeHandle>,
) -> Result<(), String> {
    runtime
        .ready()?
        .mission_controller
        .reject_plan(generation, reason)
        .await
        .map_err(|e| e.to_string())
}

/// Revert a mission that was durably recorded as merged. The workspace creates
/// a normal Git revert commit on the recorded target branch, preserving later
/// commits, and the repository records an audit-trail row. A second call on the
/// same mission is refused via that audit trail.
///
/// The durable outcome is the authorization source; this live-state check is a
/// second guard against a stale or compromised frontend racing active work.
fn validate_live_revert_state(
    mission_id: &str,
    state: Option<orchestrator::MissionState>,
) -> Result<(), String> {
    use orchestrator::MissionState;
    match state {
        None | Some(MissionState::Merged) => Ok(()),
        Some(state) => Err(format!(
            "cannot revert mission {mission_id} while its live state is {state:?}; only merged missions can be reverted"
        )),
    }
}

async fn require_merged_outcome(
    repository: &Repository,
    mission_id: &str,
) -> Result<orchestrator::MissionOutcomeDto, String> {
    let terminal = repository
        .mission_outcome(mission_id)
        .await
        .map_err(|e| format!("db: {e}"))?
        .ok_or_else(|| {
            format!("cannot revert mission {mission_id}: no durable merged outcome was recorded")
        })?;
    if terminal.state != orchestrator::MissionOutcomeState::Merged {
        return Err(format!(
            "cannot revert mission {mission_id}: durable outcome is {}",
            terminal.state.as_str()
        ));
    }
    Ok(terminal)
}

#[tauri::command]
#[specta::specta]
async fn revert_mission(
    mission_id: String,
    runtime: State<'_, RuntimeHandle>,
    app: tauri::AppHandle,
) -> Result<RevertOutcomeDto, String> {
    use orchestrator::ids::rfc3339_now;
    use orchestrator::MissionEventKind;

    let rt = runtime.ready()?;
    let controller = &rt.mission_controller;
    let repo = &rt.repository;
    static REVERT_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _revert_guard = REVERT_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;

    // Idempotency: refuse if the audit log already has an entry. The
    // workspace layer's git reset is itself idempotent (resetting to
    // the same SHA is a no-op), but the audit log is what surfaces in
    // the inbox so duplicate clicks would otherwise stack rows.
    let already = repo
        .mission_was_reverted(&mission_id)
        .await
        .map_err(|e| format!("db: {e}"))?;
    if already {
        return Err(format!("mission {mission_id} was already reverted"));
    }

    let terminal = require_merged_outcome(repo, &mission_id).await?;

    let live_state = controller.mission_state(&mission_id).await;
    validate_live_revert_state(&mission_id, live_state)?;

    let recorded_root = terminal.repo_root.as_deref().ok_or_else(|| {
        format!(
            "cannot revert mission {mission_id}: its legacy outcome has no recorded repository root"
        )
    })?;
    let repo_root = orchestrator::host_services::resolve_git_repo_root(recorded_root)
        .map_err(|e| e.to_string())?;
    if repo_root.to_string_lossy() != recorded_root {
        return Err(format!(
            "cannot revert mission {mission_id}: repository identity changed from {recorded_root:?} to {}",
            repo_root.display()
        ));
    }
    let workspace = MissionWorkspace::new(repo_root, mission_id.clone())
        .map_err(|e| format!("workspace init: {e}"))?;

    let outcome = workspace
        .revert_merged_mission(&terminal.target_ref)
        .await
        .map_err(|e| format!("revert: {e}"))?;

    // Persist the audit-trail entry only after Git succeeds.
    let now = rfc3339_now();
    repo.record_mission_revert(
        &mission_id,
        &now,
        &outcome.restored_sha,
        &outcome.pre_merge_tag,
    )
    .await
    .map_err(|e| format!("db: {e}"))?;

    // Emit MissionReverted to the frontend. The mission's
    // `MissionEventBus` is gone by this point (the runtime terminated
    // before the user clicked "revert"), so we synthesize the envelope
    // directly and ship it on the same Tauri event channel the live
    // forwarder uses. `seq` uses the current epoch-ms so the reducer's
    // ascending sort places the revert card after every prior event.
    let seq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(u64::MAX);
    let event = MissionEvent {
        mission_id: mission_id.clone(),
        seq,
        ts: now,
        kind: MissionEventKind::MissionReverted {
            restored_sha: outcome.restored_sha.clone(),
            pre_merge_tag: outcome.pre_merge_tag.clone(),
        },
    };
    if let Err(e) = MissionEventDto(event).emit(&app) {
        // Emit failure is non-fatal — the revert itself succeeded.
        // Log so the user can recover by reloading the inbox.
        tracing::error!("vigla-host: failed to emit MissionReverted: {e}");
    }

    Ok(RevertOutcomeDto {
        restored_sha: outcome.restored_sha,
        pre_merge_tag: outcome.pre_merge_tag,
    })
}

/// Remove only Vigla-owned Git artifacts retained after an aborted mission.
/// The durable outcome supplies both authorization and repository identity;
/// the caller cannot substitute its current working directory. Git cleanup is
/// idempotent, and completion is recorded only after every cleanup target
/// succeeds, so a crash or database failure can be retried safely.
#[tauri::command]
#[specta::specta]
async fn cleanup_mission_artifacts(
    mission_id: String,
    runtime: State<'_, RuntimeHandle>,
) -> Result<(), String> {
    let rt = runtime.ready()?;
    static CLEANUP_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _cleanup_guard = CLEANUP_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;

    cleanup_aborted_mission_artifacts_service(&rt.repository, &mission_id)
        .await
        .map_err(|error| error.to_string())
}

/// DTO for the `revert_mission` command. Mirrors
/// `orchestrator::mission_workspace::RevertOutcome` but lives in the
/// host crate so specta can derive the TS binding without making
/// the orchestrator depend on `specta`.
#[derive(Debug, Clone, Serialize, Type)]
pub struct RevertOutcomeDto {
    pub restored_sha: String,
    pub pre_merge_tag: String,
}

fn forward_mission_events(rx: MissionEventReceiver, app: tauri::AppHandle) {
    tokio::spawn(async move {
        run_forwarder_loop(rx, |event| {
            if let Err(e) = MissionEventDto(event).emit(&app) {
                tracing::error!("vigla-host: failed to emit MissionEventDto: {e}");
            }
        })
        .await;
    });
}

/// Drives a `MissionEventReceiver` to terminal completion, invoking
/// `emit` for every event delivered. Pulled out of
/// [`forward_mission_events`] so the lag/close handling can be
/// exercised without spinning up a Tauri `AppHandle` in tests.
///
/// Returns when either:
///   - a terminal mission kind is emitted — `Aborted`, or `MergeResolved`
///     with a `Merged`/`Discarded` resolution. `Completed` means the mission
///     is ready for the user's disposition and must not close the stream;
///     historical `Extended` records are likewise non-terminal here, or
///   - the broadcaster is dropped (`RecvError::Closed`).
///
/// `RecvError::Lagged(n)` is handled by logging the skipped count and
/// continuing the loop — the alternative (breaking) silently strands
/// the mission in the UI because the terminal event typically arrives
/// *after* the burst that caused the lag. This is the fix for C1.
async fn run_forwarder_loop<F>(mut rx: MissionEventReceiver, mut emit: F)
where
    F: FnMut(MissionEvent),
{
    use orchestrator::MissionEventKind;
    use tokio::sync::broadcast::error::RecvError;
    loop {
        match rx.recv().await {
            Ok(event) => {
                // Terminal kinds end the forwarder loop so the spawned task
                // doesn't outlive the mission. `Completed` is review-ready,
                // not terminal: the UI still needs the later Merge/Discard
                // disposition. Adding a terminal kind requires extending this match.
                let terminal = match &event.kind {
                    MissionEventKind::Aborted { .. } => true,
                    // MergeResolved is terminal ONLY for Merged/Discarded.
                    // Retain historical Extended replay compatibility. Current
                    // runtimes reject Extend before emitting this resolution.
                    MissionEventKind::MergeResolved { resolution } => {
                        !matches!(resolution, orchestrator::MergeResolution::Extended { .. })
                    }
                    _ => false,
                };
                emit(event);
                if terminal {
                    break;
                }
            }
            Err(RecvError::Lagged(n)) => {
                tracing::warn!(
                    "vigla-host: mission-event forwarder lagged, \
                     skipped {n} events — UI may be missing intermediate updates"
                );
                continue;
            }
            Err(RecvError::Closed) => break,
        }
    }
}

/// Returns true if `binary --version` exits 0 within 2 seconds.
/// Anything else — missing binary, non-zero exit, slow hang — is
/// reported as not present rather than blocking the IPC handler.
///
/// `PATH` is set from `orchestrator::resolve_user_path()` so we find
/// node-managed binaries (claude / codex / gemini) when launched as
/// a `.app` from /Applications/ — without it the GUI launcher only
/// inherits the bare /usr/bin:/bin paths.
async fn probe_cli(binary: &str) -> bool {
    let mut cmd = tokio::process::Command::new(binary);
    cmd.arg("--version");
    cmd.env("PATH", orchestrator::resolve_user_path());
    probe_command_within(cmd, 2000).await
}

/// Run `cmd` with stdio redirected to /dev/null and `kill_on_drop`
/// set, returning true iff the child exits 0 within `timeout_ms`.
///
/// `stdin = null` keeps a CLI that reads stdin from blocking forever
/// even though we redirected stdout/stderr; `kill_on_drop = true`
/// ensures the child is reaped when the timeout future is cancelled.
/// Without these flags the 2 s timeout silently leaked one orphaned
/// process per call, and `appSettings()` is invoked on every Settings
/// open and RealSpawn mount.
async fn probe_command_within(mut cmd: tokio::process::Command, timeout_ms: u64) -> bool {
    use tokio::time::{timeout, Duration};
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    match timeout(Duration::from_millis(timeout_ms), cmd.status()).await {
        Ok(Ok(status)) => status.success(),
        _ => false,
    }
}

/// `WorkerEventSink` implementation that forwards each canonical
/// event to the frontend via Tauri's typed event channel.
struct TauriEventSink {
    handle: tauri::AppHandle,
}

impl WorkerEventSink for TauriEventSink {
    fn emit(&self, event: &event_schema::Event) {
        if let Err(e) = WorkerEvent(event.clone()).emit(&self.handle) {
            tracing::error!("vigla-host: failed to emit WorkerEvent: {e}");
        }
    }
}

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            health_check,
            startup_status,
            start_mock_worker,
            start_claude_worker,
            start_codex_worker,
            start_gemini_worker,
            stop_worker,
            continue_worker,
            retry_worker,
            get_worker_diff,
            list_recent_workers,
            get_worker_info,
            switch_worker_model,
            replay_worker_events_page,
            app_settings,
            check_cli_auth,
            open_cli_login,
            open_cli_auth_docs,
            list_playbooks,
            save_playbook,
            delete_playbook,
            save_mind_map_file,
            start_mission,
            abort_mission,
            resolve_mission,
            confirm_plan,
            regenerate_plan,
            reject_plan,
            revert_mission,
            cleanup_mission_artifacts,
            memory_commands::memory_pin_note,
            memory_commands::memory_list_notes,
            memory_commands::memory_recent_events_for_mission,
            memory_commands::memory_latest_bundle_for_mission,
            inbox_commands::mission_event_visibility,
            inbox_commands::surface_inbox_notification,
            mission_history_command::list_recent_missions
        ])
        .events(collect_events![WorkerEvent, MissionEventDto])
        .typ::<orchestrator::MissionSpec>()
        .typ::<orchestrator::ResolveAction>()
        .typ::<orchestrator::MissionEvent>()
        .typ::<orchestrator::MissionEventKind>()
        .typ::<orchestrator::TaskDescriptor>()
        .typ::<orchestrator::MergeResolution>()
        // Completion Judgment: surface the typed verdict payload carried
        // opaquely on MissionEventKind::CompletionVerdictRendered.
        .typ::<orchestrator::CompletionVerdict>()
        .typ::<orchestrator::RiskBand>()
        .typ::<orchestrator::UnresolvedIssue>()
        .typ::<event_schema::Event>()
        .typ::<event_schema::EventKind>()
        .typ::<event_schema::WorkerInfo>()
        .typ::<event_schema::TaskInfo>()
        .typ::<event_schema::WorkerState>()
        .typ::<event_schema::Vendor>()
        .typ::<event_schema::StateChange>()
        .typ::<event_schema::Log>()
        .typ::<event_schema::LogLevel>()
        .typ::<event_schema::LogStream>()
        .typ::<event_schema::Progress>()
        .typ::<event_schema::FileActivity>()
        .typ::<event_schema::FileOp>()
        .typ::<event_schema::TestResult>()
        .typ::<event_schema::TestFailure>()
        .typ::<event_schema::Cost>()
        .typ::<event_schema::Dependency>()
        .typ::<event_schema::Completion>()
        .typ::<event_schema::Artifact>()
        .typ::<event_schema::ArtifactKind>()
        .typ::<event_schema::Failure>()
        .typ::<event_schema::FailureCategory>()
        .typ::<CliAuthStatusDto>()
        .typ::<CliAuthState>()
        .typ::<WorkerModelSwitchDto>()
        .typ::<StoredPlaybook>()
        // Memory Kernel (Tier-2C) — IPC request/response shapes.
        // Internal `NoteKind`/`Scope` wire shapes live in the kernel;
        // the IPC surface uses the narrower DTO enums declared in
        // `memory_commands` to keep the frontend pin form finite.
        .typ::<memory_commands::PinNoteRequest>()
        .typ::<memory_commands::PinNoteResponse>()
        .typ::<memory_commands::PinNoteKind>()
        .typ::<memory_commands::PinNoteScopeKind>()
        .typ::<memory_commands::PinNoteRejectReason>()
        .typ::<memory_commands::MemoryNoteSummaryDto>()
        .typ::<memory_commands::NoteStateFilter>()
        // Tier-2E — recent-events drawer DTOs.
        .typ::<memory_commands::MemoryEventDto>()
        .typ::<memory_commands::MemoryEventKindDto>()
        .typ::<memory_commands::MemoryBundleDto>()
        .constant("SCHEMA_VERSION", event_schema::SCHEMA_VERSION)
}

fn resolve_tracing_log_dir(
    override_dir: Option<&std::ffi::OsStr>,
    home_dir: Option<&std::ffi::OsStr>,
    temp_dir: &Path,
) -> PathBuf {
    override_dir
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home_dir
                .map(PathBuf::from)
                .map(|h| h.join("Library/Logs/Vigla"))
        })
        .unwrap_or_else(|| temp_dir.join("Vigla"))
}

fn prepare_tracing_log_dir(log_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(log_dir)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(log_dir, std::fs::Permissions::from_mode(0o700))?;
    }

    Ok(())
}

/// R1 — install a `tracing` subscriber so every diagnostic survives
/// the `.app` bundle launch (where stderr is /dev/null). Logs default to
/// `~/Library/Logs/Vigla/vigla.log.YYYY-MM-DD`; isolated harnesses can set
/// `VIGLA_LOG_DIR`. The returned `WorkerGuard` must live for the process
/// lifetime; the appender stops as soon as it drops.
fn init_tracing_subscriber() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let override_dir = std::env::var_os("VIGLA_LOG_DIR");
    let home_dir = std::env::var_os("HOME");
    let temp_dir = std::env::temp_dir();
    let log_dir = resolve_tracing_log_dir(override_dir.as_deref(), home_dir.as_deref(), &temp_dir);
    let dir_ok = prepare_tracing_log_dir(&log_dir).is_ok();

    let default_filter = "info,orchestrator=debug,vigla_host_lib=debug";

    if dir_ok {
        let file_appender = tracing_appender::rolling::daily(&log_dir, "vigla.log");
        let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
        let _ = tracing_subscriber::registry()
            .with(
                EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| EnvFilter::new(default_filter)),
            )
            .with(fmt::layer().with_writer(file_writer).with_ansi(false))
            .with(fmt::layer().with_writer(std::io::stderr).with_ansi(false))
            .try_init();
        Some(guard)
    } else {
        let _ = tracing_subscriber::registry()
            .with(
                EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| EnvFilter::new(default_filter)),
            )
            .with(fmt::layer().with_writer(std::io::stderr).with_ansi(false))
            .try_init();
        None
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    orchestrator::init();
    // The guard must live for the process lifetime — drop stops the
    // non-blocking appender immediately. `Box::leak` is the simplest
    // way to guarantee that without threading the guard through Tauri
    // state on every command boundary.
    if let Some(guard) = init_tracing_subscriber() {
        Box::leak(Box::new(guard));
    }

    let builder = specta_builder();

    #[cfg(debug_assertions)]
    builder
        .export(
            Typescript::default()
                .bigint(BigIntExportBehavior::Number)
                .header(
                    "// @ts-nocheck\n\
                     // Auto-generated by tauri-specta. Do not edit.\n\
                     /* eslint-disable */\n",
                ),
            "../src/bindings.ts",
        )
        .expect("Failed to export typescript bindings");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);

            let handle = app.handle().clone();

            // P4 — register the runtime handle synchronously so the
            // State extractor never panics. Heavy init (open DB +
            // migrations + supervisor + memory registry + retention
            // sweepers) runs on a background tokio task. Until it
            // finishes the frontend renders an "Initializing…" splash
            // gated on `vigla://startup-complete` / `startup_status`.
            app.manage(RuntimeHandle::new());

            tauri::async_runtime::spawn(async move {
                match initialize_runtime(handle.clone()).await {
                    Ok(()) => {
                        if let Err(e) = handle.emit("vigla://startup-complete", ()) {
                            tracing::error!("vigla-host: failed to emit startup-complete: {e}");
                        }
                    }
                    Err(e) => {
                        tracing::error!("vigla-host: startup failed: {e}");
                        let runtime = handle.state::<RuntimeHandle>();
                        if let Err(state_error) = runtime.fail(e.clone()) {
                            tracing::error!(
                                "vigla-host: failed to retain startup error state: {state_error}"
                            );
                        }
                        if let Err(emit_err) = handle.emit("vigla://startup-error", e) {
                            tracing::error!("vigla-host: failed to emit startup-error: {emit_err}");
                        }
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// P4 — the heavy startup sequence: open the repo (running migrations),
/// build the supervisor + memory registry + mission controller, open
/// the playbook store, and start the event-retention sweeper.
/// On success, installs the assembled [`RuntimeState`] on the managed
/// [`RuntimeHandle`]. Returns a human-readable error string on any
/// failure so it can be surfaced to the frontend as a startup-error
/// event.
async fn initialize_runtime(handle: tauri::AppHandle) -> Result<(), String> {
    let repo = Repository::open(&orchestrator::default_db_path())
        .await
        .map_err(|e| format!("open repository: {e}"))?;

    if let Err(error) = orchestrator::reconcile_disposition_journal(&repo).await {
        // Keep the application available so the operator can repair a moved or
        // inaccessible repository. The unresolved row is durable and will be
        // retried; no terminal success is fabricated.
        tracing::error!("vigla-host: disposition reconciliation incomplete: {error}");
    }

    // Install the process-wide persistent quota tracker on the same
    // database so vendor quota pauses survive a host restart and are
    // shared across missions (every MissionEventBus reads this shared
    // instance). Fail-soft: a build failure must not block startup —
    // the event bus then falls back to a per-mission in-memory tracker
    // and pauses simply won't persist across restart.
    match orchestrator::recovery::quota::VendorQuotaTracker::with_pool(repo.pool()).await {
        Ok(tracker) => orchestrator::recovery::quota::install_shared_tracker(tracker),
        Err(e) => tracing::error!(
            "vigla-host: persistent quota tracker unavailable; quota pauses \
             will not survive restart: {e}"
        ),
    }

    // Install one process-level heartbeat beside the app database. It is
    // intentionally not rooted in a guessed repository cwd: this monitor is
    // shared across missions and Finder-launched apps often inherit `/`.
    let endurance_root = endurance_storage_root();
    if let Err(e) = orchestrator::endurance::install_process_monitor(&endurance_root) {
        tracing::error!("vigla-host: endurance monitor unavailable: {e}");
    }

    let mock_harness = Supervisor::locate_mock_harness().unwrap_or_else(|e| {
        // Don't crash the app — the user can still see
        // health_check tick. They'll just get an error
        // when clicking "Start mock worker".
        tracing::warn!("vigla-host: mock-harness unavailable: {e}");
        std::path::PathBuf::new()
    });

    let sink = Arc::new(TauriEventSink {
        handle: handle.clone(),
    });
    // Repository is cheap to clone (Arc<SqlitePool>
    // inside) — Supervisor takes one handle, the host
    // keeps another for the Step-14 replay queries.
    let supervisor = Supervisor::new(repo.clone(), sink, mock_harness);
    let repo_for_retention = repo.clone();

    // Step 22 — disk-backed playbook store rooted at the
    // OS app-data dir. `app_local_data_dir()` returns
    // `~/Library/Application Support/<bundle_id>` on
    // macOS; we add `playbooks/` for our private subtree.
    let playbook_root = handle
        .path()
        .app_local_data_dir()
        .unwrap_or_else(|e| {
            tracing::warn!("vigla-host: app_local_data_dir unavailable: {e}");
            std::env::temp_dir()
        })
        .join("playbooks");
    let store = match PlaybookStore::open(playbook_root) {
        Ok(store) => store,
        Err(e) => {
            tracing::error!("vigla-host: open playbook store failed: {e}");
            // Falling back to a temp dir keeps the IPC surface working —
            // UI stays usable, user just can't persist across restarts in
            // this session. If even the fallback fails, propagate the error
            // so the frontend receives a `startup-error` event instead of a
            // panic in this spawned task that silently hangs the splash
            // (F-14).
            PlaybookStore::open(std::env::temp_dir().join("vigla-playbooks-fallback")).map_err(
                |fallback_err| {
                    format!("playbook store unavailable (primary: {e}; fallback: {fallback_err})")
                },
            )?
        }
    };

    // A2 (Tier-2G): per-repo memory kernels. Construction is
    // infallible — per-repo pool failures surface on the first-touch
    // IPC call, never at app start.
    let registry = MemoryRegistry::new();

    let controller = Arc::new(MissionController::default());
    controller.install_memory_registry(registry.clone()).await;
    controller.install_repository(repo.clone()).await;

    // Start the retention sweeper. The guard is stashed on the
    // RuntimeState so it lives for the orchestrator's lifetime and is
    // aborted on shutdown.
    let retention_guard = RetentionGuard::spawn(repo_for_retention.clone());

    // The retention guard is managed separately so its Drop runs on
    // app shutdown. (We can't store it on RuntimeState behind a
    // shared & reference because the guard's Drop needs unique
    // ownership — handing it to `handle.manage` keeps that property.)
    handle.manage(retention_guard);

    let state = RuntimeState {
        supervisor,
        repository: repo,
        playbook_store: Arc::new(store),
        mission_controller: controller,
        memory_registry: registry,
    };

    let runtime = handle.state::<RuntimeHandle>();
    runtime
        .install(state)
        .map_err(|e| format!("install runtime: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracing_log_dir_prefers_a_nonempty_override_verbatim() {
        let selected = resolve_tracing_log_dir(
            Some(std::ffi::OsStr::new("/private/vigla quota logs")),
            Some(std::ffi::OsStr::new("/Users/example")),
            Path::new("/tmp"),
        );

        assert_eq!(selected, PathBuf::from("/private/vigla quota logs"));
    }

    #[test]
    fn tracing_log_dir_ignores_an_empty_override() {
        let selected = resolve_tracing_log_dir(
            Some(std::ffi::OsStr::new("")),
            Some(std::ffi::OsStr::new("/Users/example")),
            Path::new("/tmp"),
        );

        assert_eq!(selected, PathBuf::from("/Users/example/Library/Logs/Vigla"));
    }

    #[cfg(unix)]
    #[test]
    fn tracing_log_dir_is_prepared_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let log_dir = temp.path().join("logs");
        std::fs::create_dir(&log_dir).unwrap();
        std::fs::set_permissions(&log_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        prepare_tracing_log_dir(&log_dir).unwrap();

        let mode = std::fs::metadata(&log_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn tauri_config_enforces_a_production_content_security_policy() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("valid Tauri config");
        let security = &config["app"]["security"];
        let csp = security["csp"]
            .as_object()
            .expect("production CSP must be enabled as a directive map");

        assert_eq!(csp["default-src"], "'self'");
        assert_eq!(csp["object-src"], "'none'");
        assert_eq!(csp["base-uri"], "'none'");
        assert_eq!(csp["form-action"], "'none'");
        assert_eq!(csp["connect-src"], "ipc: http://ipc.localhost");
        assert!(
            csp.values().all(|sources| {
                sources
                    .as_str()
                    .is_none_or(|sources| !sources.contains("localhost:1420"))
            }),
            "the production CSP must not permit the Vite development server"
        );

        let dev_csp = security["devCsp"]
            .as_object()
            .expect("development CSP must be configured separately");
        assert!(dev_csp["connect-src"]
            .as_str()
            .is_some_and(|sources| sources.contains("ws://localhost:1420")));
    }

    #[test]
    fn validate_mind_map_path_rejects_non_svg_extension() {
        let err = validate_mind_map_file_path("/tmp/mind-map.png").unwrap_err();
        assert!(err.contains(".svg"), "unexpected error: {err}");
    }

    #[test]
    fn validate_mind_map_path_accepts_uppercase_svg_extension() {
        let p = validate_mind_map_file_path("/tmp/mind-map.SVG").unwrap();
        assert_eq!(p.to_string_lossy(), "/tmp/mind-map.SVG");
    }

    #[test]
    fn validate_mind_map_path_rejects_parent_dir_traversal() {
        let err =
            validate_mind_map_file_path("/Users/alice/maps/../../../etc/evil.svg").unwrap_err();
        assert!(err.contains(".."), "unexpected error: {err}");
    }

    #[test]
    fn validate_mind_map_filename_rejects_renderer_paths() {
        assert!(validate_mind_map_filename("/tmp/mind-map.svg").is_err());
        assert!(validate_mind_map_filename("../mind-map.svg").is_err());
        assert_eq!(
            validate_mind_map_filename("mission-mind-map.svg").unwrap(),
            "mission-mind-map.svg"
        );
    }

    #[test]
    fn write_mind_map_to_path_rejects_non_svg_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mind-map.svg");
        let err = write_mind_map_to_path(&path, "nope").unwrap_err();
        assert!(err.contains("SVG"), "unexpected error: {err}");
    }

    #[test]
    fn write_mind_map_to_path_rejects_xml_without_an_svg_root() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mind-map.svg");
        let err = write_mind_map_to_path(&path, r#"<?xml version="1.0"?><html/>"#).unwrap_err();
        assert!(err.contains("SVG"), "unexpected error: {err}");
    }

    #[test]
    fn write_mind_map_to_path_rejects_svg_prefix_spoofing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mind-map.svg");
        let err = write_mind_map_to_path(&path, "<svgscript/>").unwrap_err();
        assert!(err.contains("SVG"), "unexpected error: {err}");
    }

    #[test]
    fn write_mind_map_to_path_writes_svg_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mind-map.svg");
        let svg = r#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg"/>"#;
        write_mind_map_to_path(&path, svg).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), svg);
    }

    #[tokio::test]
    async fn probe_command_returns_true_on_success() {
        let mut cmd = tokio::process::Command::new("/bin/echo");
        cmd.arg("ok");
        assert!(probe_command_within(cmd, 2000).await);
    }

    #[tokio::test]
    async fn probe_command_returns_false_on_missing_binary() {
        let cmd = tokio::process::Command::new("/this/binary/definitely/does/not/exist-xyz123");
        assert!(!probe_command_within(cmd, 2000).await);
    }

    #[test]
    fn cli_auth_spec_rejects_unknown_vendor() {
        let err = cli_auth_spec("opencode").unwrap_err();
        assert!(
            err.contains("unknown CLI vendor"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn shell_and_applescript_escaping_preserve_special_chars() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        assert_eq!(applescript_string("say \"hi\""), "\"say \\\"hi\\\"\"");
    }

    #[test]
    fn gemini_auth_files_detect_nonempty_known_credentials() {
        let temp = tempfile::TempDir::new().unwrap();
        assert!(!gemini_auth_files_present(temp.path()));
        std::fs::write(temp.path().join("oauth_creds.json"), "{}").unwrap();
        assert!(gemini_auth_files_present(temp.path()));
    }

    #[test]
    fn cli_auth_json_field_detection_ignores_empty_values() {
        let temp = tempfile::TempDir::new().unwrap();
        let auth = temp.path().join("auth.json");
        std::fs::write(
            &auth,
            r#"{"tokens":{"access_token":"token"},"oauthAccount":null}"#,
        )
        .unwrap();
        assert!(json_field_present(&auth, &["tokens"]));
        assert!(!json_field_present(&auth, &["oauthAccount"]));

        std::fs::write(&auth, r#"{"oauthAccount":{"email":"user@example.com"}}"#).unwrap();
        assert!(json_field_present(&auth, &["oauthAccount"]));
    }

    #[test]
    fn model_name_validation_is_free_form_but_single_line() {
        assert_eq!(
            normalize_model_name(" claude-opus-4-7 ").unwrap(),
            "claude-opus-4-7"
        );
        assert!(normalize_model_name("").is_err());
        assert!(normalize_model_name("codex\n/model other").is_err());
    }

    #[test]
    fn model_preference_requires_a_real_claude_worker() {
        let worker = event_schema::WorkerInfo {
            id: "w".into(),
            name: "claude-1".into(),
            vendor: event_schema::Vendor::Claude,
            cli_binary: "/opt/homebrew/bin/claude".into(),
            cli_version: None,
            cwd: "/tmp".into(),
            model: None,
            spawned_at: "2026-05-25T00:00:00Z".into(),
            ended_at: None,
        };
        ensure_model_preference_supported(&worker).unwrap();

        let mock_worker = event_schema::WorkerInfo {
            cli_binary: "/tmp/vigla-mock-harness".into(),
            ..worker
        };
        assert!(ensure_model_preference_supported(&mock_worker).is_err());
    }

    /// C1 regression: when the broadcaster overruns the receiver's
    /// buffer, the forwarder must surface a Lagged signal as a "skip
    /// and keep going" rather than tearing down the loop. Before the
    /// fix the `while let Ok(...)` form swallowed `Err(Lagged)` and
    /// exited, so the terminal event that arrived next was never
    /// forwarded — leaving the mission stuck in the UI.
    #[tokio::test]
    async fn forwarder_survives_lagged_burst_and_delivers_terminal() {
        use orchestrator::{MissionEvent, MissionEventKind, MissionEventReceiver};
        use std::sync::{Arc, Mutex};
        use tokio::sync::broadcast;

        // Capacity 4 keeps the test deterministic — push 16 events
        // before consuming any, guaranteeing the next recv() returns
        // Lagged.
        let (tx, rx) = broadcast::channel::<MissionEvent>(4);
        let receiver = MissionEventReceiver::for_testing(rx);

        let delivered = Arc::new(Mutex::new(Vec::<MissionEventKind>::new()));
        let delivered_for_task = Arc::clone(&delivered);

        // Spawn the forwarder before publishing so the receiver is
        // alive when the lag occurs.
        let handle = tokio::spawn(super::run_forwarder_loop(receiver, move |e| {
            delivered_for_task.lock().unwrap().push(e.kind);
        }));

        let mk = |seq: u64, kind: MissionEventKind| MissionEvent {
            mission_id: "msn-c1-regression".to_string(),
            seq,
            ts: "2026-05-20T00:00:00Z".to_string(),
            kind,
        };

        for seq in 1..=16 {
            tx.send(mk(
                seq,
                MissionEventKind::WorkerProgress {
                    worker_id: "wkr-1".to_string(),
                    note: format!("progress {seq}"),
                },
            ))
            .expect("send");
        }
        tx.send(mk(
            17,
            MissionEventKind::Completed {
                summary: "done".to_string(),
                files_changed: 1,
            },
        ))
        .expect("send review-ready event");
        tx.send(mk(
            18,
            MissionEventKind::MergeResolved {
                resolution: orchestrator::MergeResolution::Merged,
            },
        ))
        .expect("send terminal disposition");

        // Bound the wait so a regression of the underlying behaviour
        // surfaces as a test failure rather than a CI hang.
        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("forwarder did not terminate within 2s — C1 regression?")
            .expect("forwarder task panicked");

        let kinds = delivered.lock().unwrap();
        assert!(
            kinds
                .iter()
                .any(|k| matches!(k, MissionEventKind::Completed { .. })),
            "review-ready event must reach the frontend even after a lagged burst; got {kinds:?}"
        );
        assert!(
            matches!(
                kinds.last(),
                Some(MissionEventKind::MergeResolved {
                    resolution: orchestrator::MergeResolution::Merged
                })
            ),
            "the final disposition must reach the frontend after Completed; got {kinds:?}"
        );
    }

    /// Historical `Extended` and review-ready `Completed` events are not
    /// runtime-terminal. The forwarder must keep the UI subscribed until a
    /// real Merge/Discard/Abort disposition arrives.
    #[tokio::test]
    async fn forwarder_keeps_running_after_extend_resolution() {
        use orchestrator::{MergeResolution, MissionEvent, MissionEventKind, MissionEventReceiver};
        use std::sync::{Arc, Mutex};
        use tokio::sync::broadcast;

        let (tx, rx) = broadcast::channel::<MissionEvent>(16);
        let receiver = MissionEventReceiver::for_testing(rx);
        let delivered = Arc::new(Mutex::new(Vec::<MissionEventKind>::new()));
        let delivered_for_task = Arc::clone(&delivered);
        let handle = tokio::spawn(super::run_forwarder_loop(receiver, move |e| {
            delivered_for_task.lock().unwrap().push(e.kind);
        }));

        let mk = |seq: u64, kind: MissionEventKind| MissionEvent {
            mission_id: "msn-extend".to_string(),
            seq,
            ts: "2026-05-20T00:00:00Z".to_string(),
            kind,
        };

        // Extend re-opens the mission — must NOT end the forwarder.
        tx.send(mk(
            1,
            MissionEventKind::MergeResolved {
                resolution: MergeResolution::Extended { directive: None },
            },
        ))
        .expect("send extend");
        // A post-extend event must still be forwarded.
        tx.send(mk(
            2,
            MissionEventKind::WorkerProgress {
                worker_id: "wkr-1".to_string(),
                note: "after extend".to_string(),
            },
        ))
        .expect("send progress");
        // Completed means "ready for review", not runtime-terminal.
        tx.send(mk(
            3,
            MissionEventKind::Completed {
                summary: "done".to_string(),
                files_changed: 0,
            },
        ))
        .expect("send review-ready event");
        tx.send(mk(
            4,
            MissionEventKind::MergeResolved {
                resolution: MergeResolution::Merged,
            },
        ))
        .expect("send terminal");

        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("forwarder did not terminate within 2s")
            .expect("forwarder task panicked");

        let kinds = delivered.lock().unwrap();
        assert!(
            kinds
                .iter()
                .any(|k| matches!(k, MissionEventKind::WorkerProgress { .. })),
            "post-Extend events must still reach the UI; got {kinds:?}"
        );
        assert!(
            matches!(
                kinds.last(),
                Some(MissionEventKind::MergeResolved {
                    resolution: MergeResolution::Merged
                })
            ),
            "forwarder must run until a real terminal; got {kinds:?}"
        );
    }

    /// One-shot regenerator for `app/src/bindings.ts`. Off by default
    /// so `cargo test` doesn't rewrite the file on every invocation.
    /// Run with `VIGLA_REGEN_BINDINGS=1 cargo test -p vigla-host
    /// regenerate_typescript_bindings` after touching any command,
    /// event, or `.typ::<…>()` line in [`super::specta_builder`].
    #[test]
    fn regenerate_typescript_bindings() {
        if std::env::var("VIGLA_REGEN_BINDINGS").is_err() {
            return;
        }
        let builder = specta_builder();
        builder
            .export(
                Typescript::default()
                    .bigint(BigIntExportBehavior::Number)
                    .header(
                        "// @ts-nocheck\n\
                         // Auto-generated by tauri-specta. Do not edit.\n\
                         /* eslint-disable */\n",
                    ),
                "../src/bindings.ts",
            )
            .expect("Failed to export typescript bindings");
    }

    /// Every live state except `Merged` is refused. UI visibility is not a
    /// security or integrity boundary for the destructive command.
    #[test]
    fn live_revert_guard_refuses_non_merged_states() {
        use orchestrator::mission::PauseReason;
        use orchestrator::MissionState;
        let paused = Some(MissionState::Paused {
            reason: PauseReason::WaitingForQuota {
                vendor: event_schema::Vendor::Claude,
            },
        });
        let err = validate_live_revert_state("mid-test", paused)
            .expect_err("paused mission must be refused");
        assert!(
            err.contains("only merged missions can be reverted"),
            "error must state the invariant: {err}"
        );
        assert!(err.contains("mid-test"));

        for state in [
            MissionState::Created,
            MissionState::Executing,
            MissionState::PendingPlanApproval,
            MissionState::Reviewing,
            MissionState::CompletePendingMerge,
            MissionState::Attention,
            MissionState::Discarded,
            MissionState::Aborted,
        ] {
            assert!(validate_live_revert_state("mid-test", Some(state)).is_err());
        }
    }

    /// A process restarted after merge has no live runtime, while an in-process
    /// merged mission still has one; both are allowed after durable validation.
    #[test]
    fn live_revert_guard_accepts_merged_or_absent_runtime() {
        use orchestrator::MissionState;
        assert!(validate_live_revert_state("mid-1", None).is_ok());
        assert!(validate_live_revert_state("mid-2", Some(MissionState::Merged)).is_ok());
    }

    #[tokio::test]
    async fn durable_revert_guard_only_accepts_merged_outcomes() {
        let repository = Repository::open_in_memory().await.unwrap();
        let missing = require_merged_outcome(&repository, "missing")
            .await
            .unwrap_err();
        assert!(missing.contains("no durable merged outcome"));

        repository
            .record_mission_outcome(
                "discarded",
                "/repo/discarded",
                "main",
                orchestrator::MissionOutcomeState::Discarded,
                "2026-07-21T12:00:00Z",
            )
            .await
            .unwrap();
        let discarded = require_merged_outcome(&repository, "discarded")
            .await
            .unwrap_err();
        assert!(discarded.contains("durable outcome is discarded"));

        repository
            .record_mission_outcome(
                "merged",
                "/repo/merged",
                "release/v1",
                orchestrator::MissionOutcomeState::Merged,
                "2026-07-21T12:00:01Z",
            )
            .await
            .unwrap();
        let merged = require_merged_outcome(&repository, "merged").await.unwrap();
        assert_eq!(merged.target_ref, "release/v1");
    }

    #[tokio::test]
    async fn durable_cleanup_guard_only_accepts_aborted_outcomes() {
        let repository = Repository::open_in_memory().await.unwrap();
        let missing = cleanup_aborted_mission_artifacts_service(&repository, "missing")
            .await
            .unwrap_err();
        assert!(missing.to_string().contains("no durable aborted outcome"));

        repository
            .record_mission_outcome(
                "merged",
                "/repo/merged",
                "main",
                orchestrator::MissionOutcomeState::Merged,
                "2026-07-21T12:00:00Z",
            )
            .await
            .unwrap();
        let merged = cleanup_aborted_mission_artifacts_service(&repository, "merged")
            .await
            .unwrap_err();
        assert!(merged.to_string().contains("ended as merged"));

        repository
            .record_mission_outcome(
                "aborted",
                "/repo/aborted",
                "release/v1",
                orchestrator::MissionOutcomeState::Aborted,
                "2026-07-21T12:00:01Z",
            )
            .await
            .unwrap();
        let aborted = cleanup_aborted_mission_artifacts_service(&repository, "aborted")
            .await
            .unwrap_err();
        assert!(aborted
            .to_string()
            .contains("does not exist or is unreadable"));
    }

    #[tokio::test]
    async fn aborted_cleanup_removes_artifacts_and_records_completion() {
        async fn git(root: &Path, args: &[&str]) -> std::process::Output {
            tokio::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .await
                .unwrap()
        }

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(git(root, &["init", "-q", "-b", "main"])
            .await
            .status
            .success());
        assert!(git(root, &["config", "user.email", "test@example.com"])
            .await
            .status
            .success());
        assert!(git(root, &["config", "user.name", "Test"])
            .await
            .status
            .success());
        tokio::fs::write(root.join("base.txt"), "base\n")
            .await
            .unwrap();
        assert!(git(root, &["add", "base.txt"]).await.status.success());
        assert!(git(root, &["commit", "-q", "-m", "base"])
            .await
            .status
            .success());

        let canonical = orchestrator::host_services::resolve_git_repo_root(
            root.to_str().expect("temporary path must be UTF-8"),
        )
        .unwrap();
        let workspace =
            MissionWorkspace::new(canonical.clone(), "cleanup-test-0001".into()).unwrap();
        workspace.create_supervisor_branch("main").await.unwrap();
        workspace.create_supervisor_worktree().await.unwrap();
        workspace.create_worker_branch("mock-1").await.unwrap();
        workspace.create_worker_worktree("mock-1").await.unwrap();

        let repository = Repository::open_in_memory().await.unwrap();
        let canonical_str = canonical.to_string_lossy().into_owned();
        repository
            .record_mission_outcome(
                "cleanup-test-0001",
                &canonical_str,
                "main",
                orchestrator::MissionOutcomeState::Aborted,
                "2026-07-21T12:00:00Z",
            )
            .await
            .unwrap();

        cleanup_aborted_mission_artifacts_service(&repository, "cleanup-test-0001")
            .await
            .unwrap();
        assert!(!workspace.supervisor_worktree_path().exists());
        assert!(!workspace.worker_worktree_path("mock-1").unwrap().exists());
        assert!(repository
            .mission_artifacts_cleaned("cleanup-test-0001")
            .await
            .unwrap());
        let refs = git(
            &canonical,
            &[
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads/vigla/cleanup-test-0001/",
            ],
        )
        .await;
        assert!(refs.status.success());
        assert!(refs.stdout.is_empty());

        // The durable marker makes a repeat call succeed even though the Git
        // artifacts no longer exist.
        cleanup_aborted_mission_artifacts_service(&repository, "cleanup-test-0001")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn probe_command_returns_fast_when_child_outlives_timeout() {
        // Regression: without kill_on_drop, a slow child kept running
        // after the timeout returned. This test asserts the future
        // resolves within ~2x the timeout (proving the timeout fired)
        // and returns false (proving the slow path is treated as
        // "not present"). It cannot directly assert reaping without
        // PID introspection, but kill_on_drop is the only mechanism
        // that prevents the leak documented in probe_command_within.
        let mut cmd = tokio::process::Command::new("/bin/sleep");
        cmd.arg("30");
        let started = std::time::Instant::now();
        let ok = probe_command_within(cmd, 100).await;
        let elapsed = started.elapsed();
        assert!(!ok, "slow child should be reported absent");
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "timeout should fire well before sleep completes; took {elapsed:?}"
        );
    }
}
