use anyhow::{anyhow, Context, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use super::headless::{
    cleanup_idle_managed_runtimes, touch_managed_runtime, try_headless_auto_attach_runtime,
};
use super::rpc::{
    choose_active_port, discover_models, rpc_call, sample_outbound_targets, run_ss,
    ActiveRuntimeContext,
};
use crate::config::AppConfig;
use crate::translator::build_metadata;
use crate::types::RuntimeState;

pub(crate) const MANAGED_RUNTIME_ENV_FLAG: &str = "SURFWIND_MANAGED_RUNTIME";
static PORT_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"127\.0\.0\.1:(\d+)").expect("valid port regex"));
static PID_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"pid=(\d+)").expect("valid pid regex"));
static WORKSPACE_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"--workspace_id(?:=|\s+)([^\s]+)").expect("valid workspace regex"));

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

pub fn runtime_diagnostics(
    config: &AppConfig,
    workspace: Option<&str>,
    auto_launch: bool,
) -> Result<Value> {
    let runtimes = discover_runtimes(config).unwrap_or_default();
    let runtime = discover_runtime(config, workspace, auto_launch)?;
    if runtime.managed_by_surfwind {
        let _ = touch_managed_runtime(config, runtime.pid);
    }
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
        "managedBySurfwind": runtime.managed_by_surfwind,
        "autoLaunchRequested": auto_launch,
        "autoLaunchEffective": auto_launch && config.auto_launch_enabled,
        "models": models,
        "availableRuntimes": runtimes.into_iter().map(|item| {
            json!({
                "pid": item.pid,
                "ports": item.ports,
                "workspaceId": item.workspace_id,
                "managedBySurfwind": item.managed_by_surfwind,
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
    let _ = cleanup_idle_managed_runtimes(config);
    let mut candidates = discover_runtimes(config).unwrap_or_default();
    if let Some(workspace) = workspace.filter(|value| !value.trim().is_empty()) {
        let requested_root = resolve_workspace_root(Some(workspace))?;
        let requested_workspace_id = workspace_id_for_path(&requested_root);
        if let Some(runtime) = select_runtime_by_workspace_id(&candidates, &requested_workspace_id)
        {
            if runtime.managed_by_surfwind {
                let _ = touch_managed_runtime(config, runtime.pid);
            }
            return Ok(runtime);
        }
        if auto_launch && config.auto_launch_enabled {
            try_headless_auto_attach_runtime(config, &requested_root)?;
            candidates = discover_runtimes(config).unwrap_or_default();
            if let Some(runtime) =
                select_runtime_by_workspace_id(&candidates, &requested_workspace_id)
            {
                if runtime.managed_by_surfwind {
                    let _ = touch_managed_runtime(config, runtime.pid);
                }
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
        let runtime = candidates.remove(0);
        if runtime.managed_by_surfwind {
            let _ = touch_managed_runtime(config, runtime.pid);
        }
        return Ok(runtime);
    }
    let current_workspace_id = workspace_id_for_path(&resolve_workspace_root(None)?);
    if let Some(runtime) = select_runtime_by_workspace_id(&candidates, &current_workspace_id) {
        if runtime.managed_by_surfwind {
            let _ = touch_managed_runtime(config, runtime.pid);
        }
        return Ok(runtime);
    }
    Err(anyhow!(
        "multiple active language_server instances found; specify --workspace"
    ))
}

pub fn repo_root(path: &Path) -> PathBuf {
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

pub(crate) fn expand_tilde(raw: &str) -> PathBuf {
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

pub(crate) fn select_runtime_by_workspace_id(
    runtimes: &[RuntimeState],
    workspace_id: &str,
) -> Option<RuntimeState> {
    runtimes
        .iter()
        .find(|runtime| runtime.workspace_id.as_deref() == Some(workspace_id))
        .cloned()
}

pub(crate) fn runtime_env_string(keys: &[&str]) -> Option<String> {
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

pub(crate) fn discover_runtimes(config: &AppConfig) -> Result<Vec<RuntimeState>> {
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

pub(crate) fn discover_runtime_for_pid(
    config: &AppConfig,
    pid: i32,
    ports: Vec<u16>,
) -> Result<RuntimeState> {
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
    let managed_by_surfwind = env_text
        .split('\0')
        .any(|entry| entry == format!("{}=1", MANAGED_RUNTIME_ENV_FLAG))
        || pid_cmdline_contains(pid, MANAGED_RUNTIME_ENV_FLAG);
    let api_key = discover_metadata_api_key(config, &ports, &csrf)?;
    Ok(RuntimeState {
        api_key,
        ports,
        pid,
        csrf,
        workspace_id,
        managed_by_surfwind,
    })
}

fn pid_cmdline_contains(pid: i32, needle: &str) -> bool {
    let cmdline = fs::read(format!("/proc/{}/cmdline", pid)).unwrap_or_default();
    String::from_utf8_lossy(&cmdline).contains(needle)
}

pub(crate) fn discover_metadata_api_key(
    config: &AppConfig,
    ports: &[u16],
    csrf: &str,
) -> Result<String> {
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

pub(crate) fn discover_pid_ports() -> Result<Vec<(i32, Vec<u16>)>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_workspace_id_for_path() {
        let path = PathBuf::from("/home/user/my-project");
        let id = workspace_id_for_path(&path);
        assert!(id.starts_with("file_"));
        assert!(id.contains("home"));
        assert!(id.contains("user"));
        assert!(id.contains("my_project"));
    }

    #[test]
    fn test_now_iso_format() {
        let iso = now_iso();
        assert!(iso.contains('T'));
        assert!(iso.contains('Z') || iso.contains('+'));
    }
}
