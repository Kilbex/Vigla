//! Vendor profile registry.
//!
//! Profiles are the only place where Vigla records vendor-specific
//! CLI launch flags and declared side effects. The rest of the
//! orchestrator asks this module to render command arguments for a
//! role instead of scattering `claude` / `codex` / `gemini` command
//! templates through runtime code.

use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;
use thiserror::Error;

const CLAUDE_PROFILE_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/vendor_profiles/claude.json"
));
const CODEX_PROFILE_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/vendor_profiles/codex.json"
));
const GEMINI_PROFILE_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/vendor_profiles/gemini.json"
));
const ANTIGRAVITY_PROFILE_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/vendor_profiles/antigravity.json"
));
const KIRO_PROFILE_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/vendor_profiles/kiro.json"
));
const COPILOT_PROFILE_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/vendor_profiles/copilot.json"
));

const BUNDLED_PROFILE_JSON: &[(&str, &str)] = &[
    ("claude", CLAUDE_PROFILE_JSON),
    ("codex", CODEX_PROFILE_JSON),
    ("gemini", GEMINI_PROFILE_JSON),
    ("antigravity", ANTIGRAVITY_PROFILE_JSON),
    ("kiro", KIRO_PROFILE_JSON),
    ("copilot", COPILOT_PROFILE_JSON),
];

const WORKER_PLAYBOOK_PLACEHOLDER: &str = "${worker_playbook}";
const WORKER_PROMPT_PLACEHOLDER: &str = "${worker_playbook_then_prompt}";
const SUPERVISOR_PLAYBOOK_PLACEHOLDER: &str = "${supervisor_playbook}";

/// CLI vendors currently known to Vigla's profile registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum WorkerVendor {
    Claude,
    Codex,
    Gemini,
    Antigravity,
    Kiro,
    Copilot,
}

impl WorkerVendor {
    /// Parse a vendor id from user/config input. `auto` intentionally
    /// returns `None`; it is a routing mode, not a vendor.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "gemini" => Some(Self::Gemini),
            "antigravity" => Some(Self::Antigravity),
            "kiro" => Some(Self::Kiro),
            "copilot" => Some(Self::Copilot),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Antigravity => "antigravity",
            Self::Kiro => "kiro",
            Self::Copilot => "copilot",
        }
    }

    pub fn binary(self) -> &'static str {
        profile_for_vendor(self).cli_binary.as_str()
    }

    pub fn event_schema_vendor(self) -> event_schema::Vendor {
        match self {
            Self::Claude => event_schema::Vendor::Claude,
            Self::Codex => event_schema::Vendor::Codex,
            Self::Gemini => event_schema::Vendor::Gemini,
            Self::Antigravity => event_schema::Vendor::Antigravity,
            Self::Kiro => event_schema::Vendor::Kiro,
            Self::Copilot => event_schema::Vendor::Copilot,
        }
    }
}

/// Side effects Vigla can surface before/while running a vendor CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DeclaredSideEffectKind {
    PackageInstall,
    PaidApiCall,
    ExternalMutation,
    NetworkEgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclaredSideEffectMode {
    Expected,
    Possible,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredSideEffect {
    pub kind: DeclaredSideEffectKind,
    pub mode: DeclaredSideEffectMode,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VendorProfile {
    pub schema_version: u32,
    pub id: WorkerVendor,
    pub display_name: String,
    pub cli_binary: String,
    pub adapter_crate: String,
    pub commands: VendorCommands,
    pub declared_side_effects: Vec<DeclaredSideEffect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VendorCommands {
    pub supervisor: CommandTemplate,
    pub mission_worker: CommandTemplate,
    pub standalone_worker: CommandTemplate,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandTemplate {
    pub supported: bool,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRole {
    Supervisor,
    MissionWorker,
    StandaloneWorker,
}

#[derive(Debug, Clone, Copy)]
pub struct CommandVars<'a> {
    pub prompt: &'a str,
    pub cwd: Option<&'a Path>,
    pub max_turns: Option<u32>,
    pub supervisor_disallowed_tools: Option<&'a str>,
    pub supervisor_max_turns: Option<u32>,
    pub resume_session_id: Option<&'a str>,
    pub model: Option<&'a str>,
}

impl<'a> CommandVars<'a> {
    pub fn new(prompt: &'a str) -> Self {
        Self {
            prompt,
            cwd: None,
            max_turns: None,
            supervisor_disallowed_tools: None,
            supervisor_max_turns: None,
            resume_session_id: None,
            model: None,
        }
    }

    pub fn cwd(mut self, cwd: &'a Path) -> Self {
        self.cwd = Some(cwd);
        self
    }

    pub fn max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = Some(max_turns);
        self
    }

    pub fn resume_session(mut self, session_id: Option<&'a str>) -> Self {
        self.resume_session_id = session_id;
        self
    }

    pub fn model(mut self, model: Option<&'a str>) -> Self {
        self.model = model;
        self
    }

    pub fn supervisor(
        mut self,
        disallowed_tools: &'a str,
        max_turns: u32,
        resume_session_id: Option<&'a str>,
        model: Option<&'a str>,
    ) -> Self {
        self.supervisor_disallowed_tools = Some(disallowed_tools);
        self.supervisor_max_turns = Some(max_turns);
        self.resume_session_id = resume_session_id;
        self.model = model;
        self
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum VendorProfileError {
    #[error("profile {0:?} did not parse: {1}")]
    Parse(String, String),
    #[error("profile {0:?} uses unsupported schema version {1}")]
    SchemaVersion(String, u32),
    #[error("profile {0:?} id does not match file stem {1:?}")]
    IdMismatch(String, String),
    #[error("duplicate vendor profile id {0}")]
    Duplicate(String),
    #[error("profile {0} declares no side effects")]
    MissingSideEffects(String),
    #[error("profile {0} command {1:?} is unsupported")]
    UnsupportedCommand(String, CommandRole),
    #[error("profile {profile} command {role:?} is missing variable {var}")]
    MissingVariable {
        profile: String,
        role: CommandRole,
        var: &'static str,
    },
    #[error("profile {profile} command {role:?} contains unknown placeholder in {arg:?}")]
    UnknownPlaceholder {
        profile: String,
        role: CommandRole,
        arg: String,
    },
}

pub fn bundled_vendor_profiles() -> &'static [VendorProfile] {
    static PROFILES: OnceLock<Vec<VendorProfile>> = OnceLock::new();
    PROFILES.get_or_init(|| {
        parse_bundled_vendor_profiles().expect("bundled vendor profiles must be valid")
    })
}

pub fn parse_bundled_vendor_profiles() -> Result<Vec<VendorProfile>, VendorProfileError> {
    let mut profiles = Vec::new();
    let mut seen = HashSet::new();
    for (name, json) in BUNDLED_PROFILE_JSON {
        let profile = parse_profile(name, json)?;
        let id = profile.id.as_str().to_string();
        if !seen.insert(id.clone()) {
            return Err(VendorProfileError::Duplicate(id));
        }
        profiles.push(profile);
    }
    Ok(profiles)
}

pub fn profile_for_vendor(vendor: WorkerVendor) -> &'static VendorProfile {
    bundled_vendor_profiles()
        .iter()
        .find(|profile| profile.id == vendor)
        .expect("known vendor must have bundled profile")
}

pub fn render_command_args(
    profile: &VendorProfile,
    role: CommandRole,
    vars: CommandVars<'_>,
) -> Result<Vec<String>, VendorProfileError> {
    let template = command_template(profile, role);
    if !template.supported {
        return Err(VendorProfileError::UnsupportedCommand(
            profile.id.as_str().to_string(),
            role,
        ));
    }

    let mut rendered = Vec::new();
    for arg in &template.args {
        match arg.as_str() {
            "${resume_args}" => {
                if let Some(session_id) = vars.resume_session_id {
                    rendered.push("--resume".to_string());
                    rendered.push(session_id.to_string());
                }
                continue;
            }
            "${model_args}" => {
                if let Some(model) = vars.model {
                    rendered.push(model_arg_flag(profile, role)?.to_string());
                    rendered.push(model.to_string());
                }
                continue;
            }
            _ => {}
        }

        rendered.push(render_arg(profile, role, arg, vars)?);
    }
    Ok(rendered)
}

fn model_arg_flag(
    profile: &VendorProfile,
    _role: CommandRole,
) -> Result<&'static str, VendorProfileError> {
    Ok(match profile.id {
        WorkerVendor::Codex => "-m",
        WorkerVendor::Claude
        | WorkerVendor::Gemini
        | WorkerVendor::Antigravity
        | WorkerVendor::Kiro
        | WorkerVendor::Copilot => "--model",
    })
}

fn parse_profile(name: &str, json: &str) -> Result<VendorProfile, VendorProfileError> {
    let profile = serde_json::from_str::<VendorProfile>(json)
        .map_err(|e| VendorProfileError::Parse(name.to_string(), e.to_string()))?;
    if profile.schema_version != 1 {
        return Err(VendorProfileError::SchemaVersion(
            name.to_string(),
            profile.schema_version,
        ));
    }
    if profile.id.as_str() != name {
        return Err(VendorProfileError::IdMismatch(
            name.to_string(),
            profile.id.as_str().to_string(),
        ));
    }
    if profile.declared_side_effects.is_empty() {
        return Err(VendorProfileError::MissingSideEffects(name.to_string()));
    }
    Ok(profile)
}

fn command_template(profile: &VendorProfile, role: CommandRole) -> &CommandTemplate {
    match role {
        CommandRole::Supervisor => &profile.commands.supervisor,
        CommandRole::MissionWorker => &profile.commands.mission_worker,
        CommandRole::StandaloneWorker => &profile.commands.standalone_worker,
    }
}

fn render_arg(
    profile: &VendorProfile,
    role: CommandRole,
    arg: &str,
    vars: CommandVars<'_>,
) -> Result<String, VendorProfileError> {
    let mut out = arg.to_string();
    replace(&mut out, "${prompt}", vars.prompt);
    replace(
        &mut out,
        WORKER_PLAYBOOK_PLACEHOLDER,
        supervisor_adapter::WORKER_PLAYBOOK,
    );
    replace(
        &mut out,
        WORKER_PROMPT_PLACEHOLDER,
        &format!(
            "{}\n\n---\n\n{}",
            supervisor_adapter::WORKER_PLAYBOOK,
            vars.prompt
        ),
    );
    replace(
        &mut out,
        SUPERVISOR_PLAYBOOK_PLACEHOLDER,
        supervisor_adapter::PLAYBOOK,
    );

    if out.contains("${cwd}") {
        let cwd = vars
            .cwd
            .ok_or_else(|| VendorProfileError::MissingVariable {
                profile: profile.id.as_str().to_string(),
                role,
                var: "${cwd}",
            })?;
        replace(&mut out, "${cwd}", &cwd.to_string_lossy());
    }
    if out.contains("${max_turns}") {
        let max_turns = vars
            .max_turns
            .ok_or_else(|| VendorProfileError::MissingVariable {
                profile: profile.id.as_str().to_string(),
                role,
                var: "${max_turns}",
            })?
            .to_string();
        replace(&mut out, "${max_turns}", &max_turns);
    }
    if out.contains("${supervisor_disallowed_tools}") {
        let disallowed = vars.supervisor_disallowed_tools.ok_or_else(|| {
            VendorProfileError::MissingVariable {
                profile: profile.id.as_str().to_string(),
                role,
                var: "${supervisor_disallowed_tools}",
            }
        })?;
        replace(&mut out, "${supervisor_disallowed_tools}", disallowed);
    }
    if out.contains("${supervisor_max_turns}") {
        let max_turns = vars
            .supervisor_max_turns
            .ok_or_else(|| VendorProfileError::MissingVariable {
                profile: profile.id.as_str().to_string(),
                role,
                var: "${supervisor_max_turns}",
            })?
            .to_string();
        replace(&mut out, "${supervisor_max_turns}", &max_turns);
    }

    if out.contains("${") {
        return Err(VendorProfileError::UnknownPlaceholder {
            profile: profile.id.as_str().to_string(),
            role,
            arg: arg.to_string(),
        });
    }
    Ok(out)
}

fn replace(target: &mut String, needle: &str, replacement: &str) {
    if target.contains(needle) {
        *target = target.replace(needle, replacement);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn bundled_profiles_load_for_phase2_vendors() {
        let profiles = parse_bundled_vendor_profiles().expect("profiles parse");
        let ids: Vec<_> = profiles.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"claude"));
        assert!(ids.contains(&"codex"));
        assert!(ids.contains(&"antigravity"));
        assert!(ids.contains(&"kiro"));
        assert!(ids.contains(&"copilot"));

        let claude = profiles
            .iter()
            .find(|p| p.id == WorkerVendor::Claude)
            .expect("claude profile");
        assert!(claude.commands.supervisor.supported);
        assert!(claude
            .commands
            .supervisor
            .args
            .iter()
            .any(|arg| arg == SUPERVISOR_PLAYBOOK_PLACEHOLDER));

        let codex = profiles
            .iter()
            .find(|p| p.id == WorkerVendor::Codex)
            .expect("codex profile");
        assert!(!codex.commands.supervisor.supported);
        assert!(codex.commands.mission_worker.supported);
    }

    #[test]
    fn declared_side_effect_model_is_closed() {
        let bad = r#"{
          "schema_version": 1,
          "id": "claude",
          "display_name": "Bad",
          "cli_binary": "claude",
          "adapter_crate": "adapters/claude",
          "commands": {
            "supervisor": {"supported": false, "args": []},
            "mission_worker": {"supported": false, "args": []},
            "standalone_worker": {"supported": false, "args": []}
          },
          "declared_side_effects": [
            {"kind": "filesystem_write", "mode": "possible", "description": "not allowed"}
          ]
        }"#;
        let err = parse_profile("claude", bad).expect_err("unknown side effect must fail");
        assert!(
            matches!(err, VendorProfileError::Parse(_, _)),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn renders_claude_supervisor_command_from_profile() {
        let profile = profile_for_vendor(WorkerVendor::Claude);
        let args = render_command_args(
            profile,
            CommandRole::Supervisor,
            CommandVars::new("decompose this").supervisor(
                "Bash,Edit",
                8,
                Some("session-1"),
                Some("sonnet"),
            ),
        )
        .expect("render args");

        assert!(args.contains(&"--append-system-prompt".to_string()));
        assert!(args.iter().any(|arg| arg.contains("Mission Supervisor")));
        assert!(args.contains(&"--disable-slash-commands".to_string()));
        assert!(args.contains(&"--tools".to_string()));
        assert!(args.contains(&"Read,Glob,LS".to_string()));
        assert!(args.contains(&"--resume".to_string()));
        assert!(args.contains(&"session-1".to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"sonnet".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("decompose this"));
    }

    #[test]
    fn renders_codex_worker_command_with_playbook_and_cwd() {
        let profile = profile_for_vendor(WorkerVendor::Codex);
        let cwd = Path::new("/tmp/example");
        let args = render_command_args(
            profile,
            CommandRole::MissionWorker,
            CommandVars::new("implement task")
                .cwd(cwd)
                .model(Some("gpt-5.5")),
        )
        .expect("render args");

        assert_eq!(args.first().map(String::as_str), Some("exec"));
        assert!(args.contains(&"--json".to_string()));
        assert!(args.contains(&"-m".to_string()));
        assert!(args.contains(&"gpt-5.5".to_string()));
        assert!(args.contains(&"-C".to_string()));
        assert!(args.contains(&"/tmp/example".to_string()));
        assert!(args
            .last()
            .expect("prompt arg")
            .contains("Vigla Worker Playbook"));
        assert!(args.last().expect("prompt arg").contains("implement task"));
    }

    #[test]
    fn renders_gemini_mission_worker_stream_json() {
        let profile = profile_for_vendor(WorkerVendor::Gemini);
        let args = render_command_args(
            profile,
            CommandRole::MissionWorker,
            CommandVars::new("test task").cwd(Path::new("/tmp/example")),
        )
        .expect("render args");

        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
    }

    #[test]
    fn renders_antigravity_worker_with_cwd_prompt_and_model() {
        let profile = profile_for_vendor(WorkerVendor::Antigravity);
        let args = render_command_args(
            profile,
            CommandRole::MissionWorker,
            CommandVars::new("repair the failing test")
                .cwd(Path::new("/tmp/example"))
                .model(Some("gemini-3-pro")),
        )
        .expect("render args");

        assert_eq!(profile.cli_binary, "agy");
        assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(args.contains(&"--add-dir".to_string()));
        assert!(args.contains(&"/tmp/example".to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"gemini-3-pro".to_string()));
        assert_eq!(args.get(args.len() - 2).map(String::as_str), Some("-p"));
        assert_eq!(
            args.last().map(String::as_str),
            Some("repair the failing test")
        );
    }

    #[test]
    fn renders_claude_mission_worker_stream_json() {
        // The Claude adapter parses NDJSON and captures the session id
        // only from the JSON `system/init` line. A mission worker
        // launched without `--output-format stream-json` degrades every
        // line to an opaque Log event and never captures its session id
        // (breaking resume/retry). Every other real vendor's
        // mission_worker streams structured output — Claude must too.
        let profile = profile_for_vendor(WorkerVendor::Claude);
        let args = render_command_args(
            profile,
            CommandRole::MissionWorker,
            CommandVars::new("implement task").cwd(Path::new("/tmp/example")),
        )
        .expect("render args");

        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
    }

    #[test]
    fn renders_copilot_mission_worker_with_prompt_and_model() {
        let profile = profile_for_vendor(WorkerVendor::Copilot);
        let args = render_command_args(
            profile,
            CommandRole::MissionWorker,
            CommandVars::new("ship the fix")
                .cwd(Path::new("/tmp/example"))
                .model(Some("gpt-5.2")),
        )
        .expect("render args");

        assert!(args.contains(&"--allow-all".to_string()));
        assert!(args.contains(&"-C".to_string()));
        assert!(args.contains(&"/tmp/example".to_string()));
        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"json".to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"gpt-5.2".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("ship the fix"));
    }

    #[test]
    fn routing_leak_check_passes_for_runtime_sources() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        let mut leaks = Vec::new();
        for dir in ["crates/orchestrator/src", "app/src-tauri/src"] {
            scan_rs_files(&root.join(dir), &root, &mut leaks);
        }
        assert!(
            leaks.is_empty(),
            "vendor routing leaks found outside vendor_profile.rs:\n{}",
            leaks.join("\n")
        );
    }

    fn scan_rs_files(dir: &Path, root: &Path, leaks: &mut Vec<String>) {
        let entries = fs::read_dir(dir).expect("read source dir");
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                scan_rs_files(&path, root, leaks);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            if path.ends_with("vendor_profile.rs") {
                continue;
            }
            let text = fs::read_to_string(&path).expect("read source file");
            for (idx, line) in text.lines().enumerate() {
                if is_routing_leak(line) {
                    let rel = path.strip_prefix(root).unwrap_or(&path);
                    leaks.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
                }
            }
        }
    }

    fn is_routing_leak(line: &str) -> bool {
        const COMMAND_LAUNCHES: [&str; 6] = [
            "Command::new(\"claude\")",
            "Command::new(\"codex\")",
            "Command::new(\"gemini\")",
            "Command::new(\"antigravity\")",
            "Command::new(\"kiro\")",
            "Command::new(\"copilot\")",
        ];
        const CLI_FLAGS: [&str; 8] = [
            ".arg(\"--append-system-prompt\")",
            ".arg(\"--output-format\")",
            ".arg(\"--permission-mode\")",
            ".arg(\"--dangerously-skip-permissions\")",
            ".arg(\"--dangerously-bypass-approvals-and-sandbox\")",
            ".arg(\"--skip-git-repo-check\")",
            ".arg(\"--skip-trust\")",
            ".arg(\"--approval-mode\")",
        ];
        COMMAND_LAUNCHES.iter().any(|needle| line.contains(needle))
            || CLI_FLAGS.iter().any(|needle| line.contains(needle))
    }
}
