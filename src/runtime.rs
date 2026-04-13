use anyhow::{anyhow, Context, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::blocking::Client;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use walkdir::WalkDir;

use crate::config::AppConfig;
use crate::translator::build_metadata;
use crate::types::{ModelInfo, RpcResponse, RuntimeState};

const LANGUAGE_SERVER_RPC_PATH: &str = "exa.language_server_pb.LanguageServerService";
const DEFAULT_HEADLESS_API_SERVER_URL: &str = "https://server.self-serve.windsurf.com";
const DEFAULT_HEADLESS_INFERENCE_API_SERVER_URL: &str = "https://inference.codeium.com";
static HTTP_CLIENT: Lazy<Client> = Lazy::new(Client::new);
static PORT_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"127\.0\.0\.1:(\d+)").expect("valid port regex"));
static PID_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"pid=(\d+)").expect("valid pid regex"));
static WORKSPACE_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"--workspace_id(?:=|\s+)([^\s]+)").expect("valid workspace regex"));

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ActiveRuntimeContext {
    pub runtime: RuntimeState,
    pub active_port: u16,
    pub metadata: Value,
}

pub fn now_iso() -> String {
    let format = time::format_description::well_known::Rfc3339;
    time::OffsetDateTime::now_utc()
        .format(&format)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

pub fn resolve_workspace_root(workspace: Option<&str>) -> Result<PathBuf> {
    let raw = match workspace {
        Some(path) => {
            let candidate = expand_tilde(path);
            if !candidate.exists() {
                return Err(anyhow!("workspace not found: {}", path));
            }
            candidate
        }
        None => std::env::current_dir().context("current working directory")?,
    };
    let resolved = raw.canonicalize().unwrap_or(raw);
    Ok(repo_root(&resolved))
}

pub fn workspace_id_for_path(path: &Path) -> String {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut normalized = String::new();
    for ch in resolved.to_string_lossy().chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch);
        } else {
            normalized.push('_');
        }
    }
    let normalized = normalized.trim_matches('_');
    if normalized.is_empty() {
        "file".to_string()
    } else {
        format!("file_{}", normalized)
    }
}

pub fn runtime_diagnostics(config: &AppConfig, workspace: Option<&str>) -> Result<Value> {
    let runtimes = discover_runtimes(config).unwrap_or_default();
    let runtime = discover_runtime(config, workspace, true)?;
    let session_id = format!("surfwind-diagnostics-{}", now_iso());
    let metadata = build_metadata(config, &runtime.api_key, &session_id);
    let active_port = choose_active_port(config, &runtime.ports, &runtime.csrf, &metadata);
    let models = if let Some(port) = active_port {
        let active = ActiveRuntimeContext {
            runtime: runtime.clone(),
            active_port: port,
            metadata: metadata.clone(),
        };
        discover_models(config, &active)
    } else {
        config.default_models()
    };
    let requested_workspace = workspace
        .and_then(|value| resolve_workspace_root(Some(value)).ok())
        .map(|path| path.display().to_string());

    Ok(json!({
        "pid": runtime.pid,
        "ports": runtime.ports,
        "activePort": active_port,
        "workspaceId": runtime.workspace_id,
        "requestedWorkspace": requested_workspace,
        "apiKeyPresent": !runtime.api_key.is_empty(),
        "csrfPresent": !runtime.csrf.is_empty(),
        "models": models,
        "availableRuntimes": runtimes.into_iter().map(|item| {
            json!({
                "pid": item.pid,
                "ports": item.ports,
                "workspaceId": item.workspace_id,
            })
        }).collect::<Vec<_>>(),
        "outboundTargets": sample_outbound_targets(runtime.pid),
    }))
}

pub fn discover_runtime(
    config: &AppConfig,
    workspace: Option<&str>,
    auto_launch: bool,
) -> Result<RuntimeState> {
    let mut candidates = discover_runtimes(config).unwrap_or_default();
    if let Some(workspace) = workspace.filter(|value| !value.trim().is_empty()) {
        let requested_root = resolve_workspace_root(Some(workspace))?;
        let requested_workspace_id = workspace_id_for_path(&requested_root);
        if let Some(runtime) = select_runtime_by_workspace_id(&candidates, &requested_workspace_id)
        {
            return Ok(runtime);
        }
        if auto_launch && config.auto_launch_enabled {
            try_headless_auto_attach_runtime(config, &requested_root)?;
            candidates = discover_runtimes(config).unwrap_or_default();
            if let Some(runtime) =
                select_runtime_by_workspace_id(&candidates, &requested_workspace_id)
            {
                return Ok(runtime);
            }
            return Err(anyhow!(
                "headless auto-attach did not attach workspace: {}",
                requested_root.display()
            ));
        }
        return Err(anyhow!(
            "no active language_server for workspace: {}",
            requested_root.display()
        ));
    }

    if candidates.is_empty() {
        return Err(anyhow!("compatible language_server not found"));
    }
    if candidates.len() == 1 {
        return Ok(candidates.remove(0));
    }
    let current_workspace_id = workspace_id_for_path(&resolve_workspace_root(None)?);
    if let Some(runtime) = select_runtime_by_workspace_id(&candidates, &current_workspace_id) {
        return Ok(runtime);
    }
    Err(anyhow!(
        "multiple active language_server instances found; specify --workspace"
    ))
}

pub fn choose_active_port(
    config: &AppConfig,
    ports: &[u16],
    csrf: &str,
    metadata: &Value,
) -> Option<u16> {
    for round in 0..3 {
        for port in ports {
            if rpc_call(
                config,
                *port,
                csrf,
                "GetUserStatus",
                &json!({ "metadata": metadata }),
            )
            .status
                == 200
            {
                return Some(*port);
            }
        }
        if round < 2 {
            thread::sleep(Duration::from_millis(150));
        }
    }
    None
}

pub fn rpc_call(
    config: &AppConfig,
    port: u16,
    csrf: &str,
    method: &str,
    body: &Value,
) -> RpcResponse {
    let url = format!(
        "http://127.0.0.1:{}/{}/{}",
        port, LANGUAGE_SERVER_RPC_PATH, method
    );
    let response = HTTP_CLIENT
        .post(url)
        .header("content-type", "application/json")
        .header("x-codeium-csrf-token", csrf)
        .timeout(Duration::from_secs_f64(config.rpc_timeout_sec))
        .body(body.to_string())
        .send();

    match response {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let text = resp.text().unwrap_or_default();
            RpcResponse { status, text }
        }
        Err(err) => {
            if let Some(status) = err.status() {
                let text = err.to_string();
                RpcResponse {
                    status: status.as_u16(),
                    text,
                }
            } else {
                RpcResponse {
                    status: 0,
                    text: err.to_string(),
                }
            }
        }
    }
}

pub fn sample_outbound_targets(pid: i32) -> Vec<Value> {
    let text = match run_ss(&["-tnpH"]) {
        Ok(text) => text,
        Err(_) => return Vec::new(),
    };
    let mut counts: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for line in text.lines() {
        if !line.contains(&format!("pid={}", pid)) {
            continue;
        }
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }
        let state = parts[0].to_string();
        let peer = parts[4].to_string();
        let entry = counts.entry(peer).or_insert((state, 0));
        entry.1 += 1;
    }
    counts
        .into_iter()
        .map(|(peer, (state, count))| json!({ "peer": peer, "state": state, "count": count }))
        .collect()
}

pub fn discover_models(config: &AppConfig, active: &ActiveRuntimeContext) -> Vec<ModelInfo> {
    let response = rpc_call(
        config,
        active.active_port,
        &active.runtime.csrf,
        "GetUserSettings",
        &json!({ "metadata": active.metadata }),
    );
    if response.status != 200 {
        return config.default_models();
    }

    let payload: Value = serde_json::from_str(&response.text).unwrap_or(Value::Null);
    let discovered_rows = collect_model_objects(&payload);
    if !discovered_rows.is_empty() {
        let mut deduped: BTreeMap<String, ModelInfo> = BTreeMap::new();
        for row in discovered_rows {
            let key = row.id.clone();
            let replace = deduped
                .get(&key)
                .map(|current| score_model(current) <= score_model(&row))
                .unwrap_or(true);
            if replace {
                deduped.insert(key, decorate_model(row));
            }
        }
        return deduped.into_values().collect();
    }

    let mut ids = BTreeSet::new();
    collect_model_uids(&payload, &mut ids);
    if ids.is_empty() {
        return config.default_models();
    }
    ids.into_iter()
        .map(|id| {
            decorate_model(ModelInfo {
                id,
                object: "model".to_string(),
                owned_by: "windsurf-local".to_string(),
                label: None,
                provider: None,
            })
        })
        .collect()
}

#[allow(dead_code)]
pub fn prepare_active_runtime_context(
    config: &AppConfig,
    workspace: Option<&str>,
) -> Result<ActiveRuntimeContext> {
    let runtime = discover_runtime(config, workspace, true)?;
    let session_id = format!("surfwind-{}", uuid::Uuid::new_v4());
    let metadata = build_metadata(config, &runtime.api_key, &session_id);
    let active_port = choose_active_port(config, &runtime.ports, &runtime.csrf, &metadata)
        .ok_or_else(|| anyhow!("no working language_server port"))?;
    Ok(ActiveRuntimeContext {
        runtime,
        active_port,
        metadata,
    })
}

fn repo_root(path: &Path) -> PathBuf {
    let candidate = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().unwrap_or(path).to_path_buf()
    };
    let markers = [
        ".git",
        ".hg",
        ".svn",
        "pyproject.toml",
        "package.json",
        "Cargo.toml",
        "go.mod",
        "Makefile",
    ];
    for current in std::iter::once(candidate.as_path()).chain(candidate.ancestors()) {
        if markers.iter().any(|marker| current.join(marker).exists()) {
            return current.to_path_buf();
        }
    }
    candidate
}

fn expand_tilde(raw: &str) -> PathBuf {
    if let Some(stripped) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    if raw == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    }
    PathBuf::from(raw)
}

fn select_runtime_by_workspace_id(
    runtimes: &[RuntimeState],
    workspace_id: &str,
) -> Option<RuntimeState> {
    runtimes
        .iter()
        .find(|runtime| runtime.workspace_id.as_deref() == Some(workspace_id))
        .cloned()
}

fn runtime_env_string(keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn find_language_server_binary(config: &AppConfig) -> Option<PathBuf> {
    if let Some(raw) = runtime_env_string(&[
        "SURFWIND_LANGUAGE_SERVER_PATH",
        "WINDSURF_LANGUAGE_SERVER_PATH",
    ]) {
        let path = PathBuf::from(raw);
        if path.is_file() {
            return Some(path);
        }
    }

    if let Ok(raw) = std::env::var("CODEIUM_EDITOR_APP_ROOT") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed).join("extensions/windsurf/bin/language_server_linux_x64");
            if path.is_file() {
                return Some(path);
            }
        }
    }

    let bin_root = config.state_dir.parent().map(|path| path.join("bin"))?;
    let mut candidates: Vec<_> = fs::read_dir(bin_root)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join("extensions/windsurf/bin/language_server_linux_x64"))
        .filter(|path| path.is_file())
        .collect();
    candidates.sort_by_key(|path| fs::metadata(path).and_then(|meta| meta.modified()).ok());
    candidates.reverse();
    candidates.into_iter().next()
}

fn find_headless_database_dir() -> Option<PathBuf> {
    if let Some(raw) = runtime_env_string(&["SURFWIND_DATABASE_DIR", "WINDSURF_DATABASE_DIR"]) {
        let path = expand_tilde(&raw);
        if path.exists() {
            return Some(path);
        }
    }

    let base = dirs::home_dir()?.join(".codeium/windsurf/database");
    if !base.exists() {
        return None;
    }
    let mut candidates: Vec<_> = fs::read_dir(&base)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    candidates.sort_by_key(|path| fs::metadata(path).and_then(|meta| meta.modified()).ok());
    candidates.reverse();
    candidates.into_iter().next().or(Some(base))
}

fn headless_api_server_url() -> String {
    runtime_env_string(&["SURFWIND_API_SERVER_URL", "WINDSURF_API_SERVER_URL"])
        .unwrap_or_else(|| DEFAULT_HEADLESS_API_SERVER_URL.to_string())
}

fn headless_inference_api_server_url() -> String {
    runtime_env_string(&[
        "SURFWIND_INFERENCE_API_SERVER_URL",
        "WINDSURF_INFERENCE_API_SERVER_URL",
    ])
    .unwrap_or_else(|| DEFAULT_HEADLESS_INFERENCE_API_SERVER_URL.to_string())
}

fn tail_log(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    let chars: Vec<_> = text.chars().collect();
    let start = chars.len().saturating_sub(2000);
    chars[start..].iter().collect()
}

fn wait_for_workspace_runtime(
    config: &AppConfig,
    requested_workspace_id: &str,
    deadline: Instant,
) -> bool {
    while Instant::now() <= deadline {
        if let Ok(runtimes) = discover_runtimes(config) {
            if select_runtime_by_workspace_id(&runtimes, requested_workspace_id).is_some() {
                return true;
            }
        }
        thread::sleep(Duration::from_millis(
            config.auto_launch_poll_interval_ms.max(50),
        ));
    }
    false
}

fn spawn_headless_runtime(
    config: &AppConfig,
    workspace_root: &Path,
    requested_workspace_id: &str,
) -> Result<PathBuf> {
    let language_server = find_language_server_binary(config).ok_or_else(|| {
        anyhow!(
            "headless language_server binary not found; set SURFWIND_LANGUAGE_SERVER_PATH to override"
        )
    })?;
    let database_dir = find_headless_database_dir().ok_or_else(|| {
        anyhow!(
            "headless database directory not found; set SURFWIND_DATABASE_DIR to override"
        )
    })?;
    let csrf_token = uuid::Uuid::new_v4().to_string();
    let log_path = config
        .paths
        .logs_dir
        .join(format!("headless-{}.log", requested_workspace_id));
    let stderr_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("open {}", log_path.display()))?;

    let mut child = Command::new(language_server);
    child
        .arg("--run_child")
        .arg("--api_server_url")
        .arg(headless_api_server_url())
        .arg("--enable_lsp")
        .arg("--ide_name")
        .arg(&config.metadata_ide_name)
        .arg("--random_port")
        .arg("--inference_api_server_url")
        .arg(headless_inference_api_server_url())
        .arg("--database_dir")
        .arg(database_dir)
        .arg("--enable_index_service")
        .arg("--enable_local_search")
        .arg("--search_max_workspace_file_count")
        .arg("5000")
        .arg("--indexed_files_retention_period_days")
        .arg("30")
        .arg("--workspace_id")
        .arg(requested_workspace_id)
        .arg("--codeium_dir")
        .arg(".codeium/windsurf")
        .arg("--csrf_token")
        .arg(&csrf_token)
        .env("WINDSURF_CSRF_TOKEN", &csrf_token)
        .env_remove("VSCODE_IPC_HOOK_CLI")
        .env_remove("VSCODE_CLIENT_COMMAND")
        .env_remove("VSCODE_CLIENT_COMMAND_CWD")
        .env_remove("VSCODE_CLI_AUTHORITY")
        .current_dir(workspace_root)
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file));

    child.spawn().context("spawn headless language_server")?;
    thread::sleep(Duration::from_millis(500));
    Ok(log_path)
}

fn try_headless_auto_attach_runtime(config: &AppConfig, workspace_root: &Path) -> Result<()> {
    let requested_workspace_id = workspace_id_for_path(workspace_root);
    let initial_deadline = Instant::now() + Duration::from_millis(200);
    if wait_for_workspace_runtime(config, &requested_workspace_id, initial_deadline) {
        return Ok(());
    }

    let log_path = spawn_headless_runtime(config, workspace_root, &requested_workspace_id)?;
    let poll_deadline =
        Instant::now() + Duration::from_secs_f64(config.auto_launch_timeout_sec.max(1.0));
    if wait_for_workspace_runtime(config, &requested_workspace_id, poll_deadline) {
        return Ok(());
    }

    Err(anyhow!(
        "headless auto-attach did not attach workspace: {}; log tail: {}",
        workspace_root.display(),
        tail_log(&log_path)
    ))
}

fn discover_runtimes(config: &AppConfig) -> Result<Vec<RuntimeState>> {
    let ports_by_pid = discover_pid_ports()?;
    let mut candidates = Vec::new();
    let mut last_error = None;

    for (pid, ports) in ports_by_pid {
        match discover_runtime_for_pid(config, pid, ports) {
            Ok(runtime) => candidates.push(runtime),
            Err(err) => last_error = Some(err),
        }
    }

    if !candidates.is_empty() {
        return Ok(candidates);
    }
    if let Some(err) = last_error {
        Err(anyhow!("compatible language_server not found: {}", err))
    } else {
        Err(anyhow!("compatible language_server not found"))
    }
}

fn discover_runtime_for_pid(config: &AppConfig, pid: i32, ports: Vec<u16>) -> Result<RuntimeState> {
    let env_raw = fs::read(format!("/proc/{}/environ", pid))
        .with_context(|| format!("read /proc/{}/environ", pid))?;
    let env_text = String::from_utf8_lossy(&env_raw);
    let csrf = env_text
        .split('\0')
        .find_map(|entry| entry.strip_prefix("WINDSURF_CSRF_TOKEN="))
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow!("WINDSURF_CSRF_TOKEN not found"))?;
    let cmdline_raw = fs::read(format!("/proc/{}/cmdline", pid))
        .with_context(|| format!("read /proc/{}/cmdline", pid))?;
    let cmdline = String::from_utf8_lossy(&cmdline_raw).replace('\0', " ");
    let workspace_id = WORKSPACE_REGEX
        .captures(&cmdline)
        .and_then(|caps| caps.get(1))
        .map(|value| value.as_str().trim().to_string())
        .filter(|value| !value.is_empty());
    let api_key = discover_metadata_api_key(config, &ports, &csrf)?;
    Ok(RuntimeState {
        api_key,
        ports,
        pid,
        csrf,
        workspace_id,
    })
}

fn discover_metadata_api_key(config: &AppConfig, ports: &[u16], csrf: &str) -> Result<String> {
    let mut candidates = Vec::new();
    if let Some(key) = config
        .metadata_api_key
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        candidates.push(key.trim().to_string());
    }
    candidates.extend(candidate_api_keys_from_state_dir(&config.state_dir));
    candidates.extend(candidate_api_keys_from_user_settings(
        &config.user_settings_path,
    ));
    dedupe_in_place(&mut candidates);

    for candidate in candidates {
        let metadata = build_metadata(
            config,
            &candidate,
            &format!("surfwind-auth-probe-{}", now_iso()),
        );
        for port in ports {
            let response = rpc_call(
                config,
                *port,
                csrf,
                "GetUserStatus",
                &json!({ "metadata": metadata }),
            );
            if response.status == 200 {
                return Ok(candidate);
            }
        }
    }
    Err(anyhow!("unable to discover local Windsurf login metadata; set SURFWIND_METADATA_API_KEY to override"))
}

fn candidate_api_keys_from_user_settings(path: &Path) -> Vec<String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Vec::new(),
    };
    let mut candidates = Vec::new();
    let mut current = Vec::new();
    for byte in bytes {
        if (0x20..=0x7E).contains(&byte) {
            current.push(byte);
        } else {
            if current.len() >= 20 {
                if let Ok(text) = String::from_utf8(current.clone()) {
                    if is_likely_api_key(&text) {
                        candidates.push(text);
                    }
                }
            }
            current.clear();
        }
    }
    if current.len() >= 20 {
        if let Ok(text) = String::from_utf8(current) {
            if is_likely_api_key(&text) {
                candidates.push(text);
            }
        }
    }
    dedupe_in_place(&mut candidates);
    candidates
}

fn candidate_api_keys_from_state_dir(state_dir: &Path) -> Vec<String> {
    let root = state_dir.join("User").join("globalStorage");
    if !root.is_dir() {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    for entry in WalkDir::new(root).into_iter().flatten() {
        if entry.file_name() != "accounts.json" {
            continue;
        }
        let text = match fs::read_to_string(entry.path()) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let payload: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let items: Vec<Value> = match payload {
            Value::Array(items) => items,
            Value::Object(mut obj) => match obj.remove("accounts") {
                Some(Value::Array(items)) => items,
                _ => vec![Value::Object(obj)],
            },
            _ => Vec::new(),
        };
        let mut dicts: Vec<Map<String, Value>> = items
            .into_iter()
            .filter_map(|item| item.as_object().cloned())
            .collect();
        dicts.sort_by_key(|item| {
            if item.get("isActive").and_then(Value::as_bool) == Some(true) {
                0
            } else {
                1
            }
        });
        for item in dicts {
            if let Some(api_key) = item.get("apiKey").and_then(Value::as_str) {
                let trimmed = api_key.trim();
                if !trimmed.is_empty() {
                    candidates.push(trimmed.to_string());
                }
            }
        }
    }
    dedupe_in_place(&mut candidates);
    candidates
}

fn discover_pid_ports() -> Result<Vec<(i32, Vec<u16>)>> {
    let text = run_ss(&["-ltnpH"])?;
    let mut ports_by_pid: BTreeMap<i32, BTreeSet<u16>> = BTreeMap::new();
    for line in text.lines() {
        if !line.contains("language_server") || !line.contains("127.0.0.1:") {
            continue;
        }
        let port = PORT_REGEX
            .captures(line)
            .and_then(|caps| caps.get(1))
            .and_then(|value| value.as_str().parse::<u16>().ok());
        let pid = PID_REGEX
            .captures(line)
            .and_then(|caps| caps.get(1))
            .and_then(|value| value.as_str().parse::<i32>().ok());
        if let (Some(port), Some(pid)) = (port, pid) {
            ports_by_pid.entry(pid).or_default().insert(port);
        }
    }
    if ports_by_pid.is_empty() {
        return Err(anyhow!("language_server listening port not found"));
    }
    Ok(ports_by_pid
        .into_iter()
        .map(|(pid, ports)| (pid, ports.into_iter().collect()))
        .collect())
}

fn run_ss(args: &[&str]) -> Result<String> {
    let output = if which::which("rtk").is_ok() {
        Command::new("rtk")
            .arg("ss")
            .args(args)
            .output()
            .context("run rtk ss")?
    } else {
        Command::new("ss").args(args).output().context("run ss")?
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(stderr.trim().to_string()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn dedupe_in_place(values: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

fn is_likely_api_key(value: &str) -> bool {
    value.len() >= 24
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':' | '/' | '+' | '=')
        })
}

fn collect_model_objects(value: &Value) -> Vec<ModelInfo> {
    let mut rows = Vec::new();
    collect_model_objects_inner(value, &mut rows);
    rows
}

fn collect_model_objects_inner(value: &Value, rows: &mut Vec<ModelInfo>) {
    match value {
        Value::Object(map) => {
            if let Some(model_uid) = map
                .get("modelUid")
                .and_then(Value::as_str)
                .filter(|item| is_likely_selectable_model_uid(item))
            {
                rows.push(ModelInfo {
                    id: model_uid.to_string(),
                    object: "model".to_string(),
                    owned_by: "windsurf-local".to_string(),
                    label: map
                        .get("label")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    provider: map
                        .get("provider")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                });
            }
            for nested in map.values() {
                collect_model_objects_inner(nested, rows);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_model_objects_inner(item, rows);
            }
        }
        _ => {}
    }
}

fn collect_model_uids(value: &Value, found: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, nested) in map {
                if let Some(text) = nested.as_str() {
                    if is_likely_selectable_model_uid(text) {
                        found.insert(text.to_string());
                    }
                    if key.to_ascii_lowercase().contains("model")
                        && is_likely_selectable_model_uid(text)
                    {
                        found.insert(text.to_string());
                    }
                }
                collect_model_uids(nested, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_model_uids(item, found);
            }
        }
        _ => {}
    }
}

fn score_model(row: &ModelInfo) -> usize {
    usize::from(row.label.is_some()) + usize::from(row.provider.is_some())
}

fn decorate_model(mut row: ModelInfo) -> ModelInfo {
    if row
        .provider
        .as_deref()
        .map(|value| value.starts_with("MODEL_PROVIDER_"))
        .unwrap_or(false)
    {
        row.provider = row.provider.clone().map(|value| {
            value
                .trim_start_matches("MODEL_PROVIDER_")
                .to_ascii_lowercase()
        });
    } else if row
        .provider
        .as_deref()
        .map(|value| value.trim().is_empty())
        .unwrap_or(true)
    {
        row.provider = derive_provider_name(&row.id);
    }
    if row
        .label
        .as_deref()
        .map(|value| value.trim().is_empty())
        .unwrap_or(true)
    {
        row.label = Some(derive_model_label(&row.id));
    }
    row
}

fn derive_provider_name(model_id: &str) -> Option<String> {
    let lower = model_id.to_ascii_lowercase();
    if lower.contains("kimi") || lower.contains("moonshot") {
        Some("moonshot".to_string())
    } else if lower.contains("claude") {
        Some("anthropic".to_string())
    } else if lower.contains("gemini") {
        Some("google".to_string())
    } else if lower.contains("glm") {
        Some("glm".to_string())
    } else if lower.contains("grok") || lower.contains("xai") {
        Some("xai".to_string())
    } else if lower.contains("gpt") || lower.contains("o3") {
        Some("openai".to_string())
    } else if lower.contains("swe") || lower.contains("windsurf") {
        Some("windsurf".to_string())
    } else {
        None
    }
}

fn derive_model_label(model_id: &str) -> String {
    let mut text = model_id.trim().to_string();
    if let Some(stripped) = text.strip_prefix("MODEL_") {
        text = stripped.to_string();
    }
    for (source, target) in [
        ("CHAT_GPT_", "GPT "),
        ("CHAT_O3", "O3"),
        ("GOOGLE_GEMINI_", "Gemini "),
        ("XAI_GROK_", "Grok "),
        ("CLAUDE_", "Claude "),
        ("GLM_", "GLM "),
        ("SWE_", "SWE "),
    ] {
        if let Some(stripped) = text.strip_prefix(source) {
            text = format!("{}{}", target, stripped);
            break;
        }
    }
    let normalized_text = text.replace('_', " ").replace('-', " ");
    let parts: Vec<_> = normalized_text.split_whitespace().collect();
    let normalized: Vec<String> = parts
        .into_iter()
        .map(|part| {
            let upper = part.to_ascii_uppercase();
            if ["GPT", "O3", "GLM", "SWE", "BYOK", "XAI"].contains(&upper.as_str()) {
                upper
            } else {
                match part.to_ascii_lowercase().as_str() {
                    "kimi" => "Kimi".to_string(),
                    "gemini" => "Gemini".to_string(),
                    "claude" => "Claude".to_string(),
                    "grok" => "Grok".to_string(),
                    other => {
                        let mut chars = other.chars();
                        match chars.next() {
                            Some(first) => {
                                format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
                            }
                            None => String::new(),
                        }
                    }
                }
            }
        })
        .collect();
    let joined = normalized.join(" ");
    if joined.is_empty() {
        model_id.to_string()
    } else {
        joined
    }
}

fn is_likely_selectable_model_uid(model_uid: &str) -> bool {
    let candidate = model_uid.trim();
    if candidate.is_empty() {
        return false;
    }
    for blocked in [
        "COST_TIER",
        "DIMENSION_KIND",
        "PRIORITY",
        "MINIMAL",
        "PRICING_TYPE",
        "PROVIDER_",
        "MODEL_TYPE_",
    ] {
        if candidate.contains(blocked) {
            return false;
        }
    }
    if candidate.starts_with("MODEL_PRIVATE_") {
        return false;
    }
    if candidate.starts_with("MODEL_") {
        return true;
    }
    candidate.chars().any(|ch| ch.is_ascii_lowercase()) && candidate.contains('-')
}
