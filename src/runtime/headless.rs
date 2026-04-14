use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::discovery::{
    discover_metadata_api_key, discover_pid_ports, discover_runtime, discover_runtimes,
    expand_tilde, now_iso, runtime_env_string, select_runtime_by_workspace_id,
    workspace_id_for_path, MANAGED_RUNTIME_ENV_FLAG,
};
use super::rpc::{choose_active_port, rpc_call, ActiveRuntimeContext};
use crate::config::AppConfig;
use crate::translator::build_metadata;
use crate::types::RunRecord;

const DEFAULT_HEADLESS_API_SERVER_URL: &str = "https://server.self-serve.windsurf.com";
const DEFAULT_HEADLESS_INFERENCE_API_SERVER_URL: &str = "https://inference.codeium.com";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ManagedRuntimeRecord {
    pid: i32,
    workspace_id: String,
    workspace_root: String,
    started_at: String,
    last_used_at: String,
}

#[derive(Clone, Debug)]
struct SpawnedRuntime {
    pid: i32,
    log_path: PathBuf,
}

fn load_managed_runtime_records(config: &AppConfig) -> Vec<ManagedRuntimeRecord> {
    let text = fs::read_to_string(&config.paths.managed_runtimes_path).unwrap_or_default();
    serde_json::from_str::<Vec<ManagedRuntimeRecord>>(&text).unwrap_or_default()
}

fn save_managed_runtime_records(
    config: &AppConfig,
    records: &[ManagedRuntimeRecord],
) -> Result<()> {
    if records.is_empty() {
        if config.paths.managed_runtimes_path.exists() {
            let _ = fs::remove_file(&config.paths.managed_runtimes_path);
        }
        return Ok(());
    }
    fs::write(
        &config.paths.managed_runtimes_path,
        serde_json::to_string_pretty(records)?,
    )
    .with_context(|| format!("write {}", config.paths.managed_runtimes_path.display()))
}

fn pid_exists(pid: i32) -> bool {
    PathBuf::from(format!("/proc/{}", pid)).exists()
}

fn managed_runtime_record_for_workspace(
    config: &AppConfig,
    workspace_id: &str,
) -> Option<ManagedRuntimeRecord> {
    load_managed_runtime_records(config)
        .into_iter()
        .find(|record| record.workspace_id == workspace_id)
}

fn upsert_managed_runtime_record(config: &AppConfig, record: ManagedRuntimeRecord) -> Result<()> {
    let mut records: Vec<_> = load_managed_runtime_records(config)
        .into_iter()
        .filter(|item| item.workspace_id != record.workspace_id && item.pid != record.pid)
        .filter(|item| pid_exists(item.pid))
        .collect();
    records.push(record);
    save_managed_runtime_records(config, &records)
}

fn prune_stale_managed_runtime_records(config: &AppConfig) -> Result<Vec<ManagedRuntimeRecord>> {
    let records: Vec<_> = load_managed_runtime_records(config)
        .into_iter()
        .filter(|record| pid_exists(record.pid))
        .collect();
    save_managed_runtime_records(config, &records)?;
    Ok(records)
}

pub(crate) fn parse_iso_timestamp(raw: &str) -> Option<time::OffsetDateTime> {
    let format = time::format_description::well_known::Rfc3339;
    time::OffsetDateTime::parse(raw, &format).ok()
}

fn active_port_for_pid(config: &AppConfig, pid: i32) -> Option<(u16, String)> {
    let ports = discover_pid_ports()
        .ok()?
        .into_iter()
        .find(|(item_pid, _)| *item_pid == pid)
        .map(|(_, ports)| ports)?;
    if ports.is_empty() {
        return None;
    }
    let env_raw = fs::read(format!("/proc/{}/environ", pid)).ok()?;
    let env_text = String::from_utf8_lossy(&env_raw);
    let csrf = env_text
        .split('\0')
        .find_map(|entry| entry.strip_prefix("WINDSURF_CSRF_TOKEN="))?
        .to_string();
    let api_key = discover_metadata_api_key(config, &ports, &csrf).ok()?;
    let metadata = build_metadata(
        config,
        &api_key,
        &format!("surfwind-managed-probe-{}", now_iso()),
    );
    let active_port = choose_active_port(config, &ports, &csrf, &metadata)?;
    Some((active_port, csrf))
}

fn is_upstream_run_still_running(config: &AppConfig, pid: i32, record: &RunRecord) -> bool {
    let Some(cascade_id) = record.cascade_id.as_deref() else {
        return true;
    };
    let Some((active_port, csrf)) = active_port_for_pid(config, pid) else {
        return true;
    };
    let response = rpc_call(
        config,
        active_port,
        &csrf,
        "GetCascadeTrajectory",
        &json!({ "cascadeId": cascade_id }),
    );
    if response.status != 200 {
        return true;
    }
    let payload = serde_json::from_str::<Value>(&response.text).unwrap_or(Value::Null);
    matches!(
        payload.get("status").and_then(Value::as_str),
        Some("CASCADE_RUN_STATUS_RUNNING")
    )
}

fn has_local_running_runs_for_pid(config: &AppConfig, pid: i32) -> bool {
    let run_dir = config.run_store_dir();
    let Ok(entries) = fs::read_dir(run_dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<RunRecord>(&text) else {
            continue;
        };
        let runtime_pid = record
            .summary
            .get("runtimePid")
            .and_then(Value::as_i64)
            .map(|value| value as i32);
        if record.status == "running"
            && runtime_pid == Some(pid)
            && is_upstream_run_still_running(config, pid, &record)
        {
            return true;
        }
    }
    false
}

fn is_idle_managed_runtime(config: &AppConfig, record: &ManagedRuntimeRecord) -> bool {
    if !pid_exists(record.pid) {
        return false;
    }
    if has_local_running_runs_for_pid(config, record.pid) {
        return false;
    }
    let Some(last_used_at) = parse_iso_timestamp(&record.last_used_at) else {
        return false;
    };
    let idle_for = time::OffsetDateTime::now_utc() - last_used_at;
    idle_for >= time::Duration::minutes(10)
}

fn terminate_managed_runtime(pid: i32) {
    if !pid_exists(pid) {
        return;
    }
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    for _ in 0..10 {
        if !pid_exists(pid) {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let _ = Command::new("kill")
        .arg("-KILL")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub fn cleanup_idle_managed_runtimes(config: &AppConfig) -> Result<()> {
    let records = prune_stale_managed_runtime_records(config)?;
    let mut kept = Vec::new();
    for record in records {
        if is_idle_managed_runtime(config, &record) {
            terminate_managed_runtime(record.pid);
        }
        if pid_exists(record.pid) {
            kept.push(record);
        }
    }
    save_managed_runtime_records(config, &kept)
}

pub fn touch_managed_runtime(config: &AppConfig, pid: i32) -> Result<()> {
    let mut records = prune_stale_managed_runtime_records(config)?;
    let now = now_iso();
    for record in &mut records {
        if record.pid == pid {
            record.last_used_at = now.clone();
        }
    }
    save_managed_runtime_records(config, &records)
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
            let path =
                PathBuf::from(trimmed).join("extensions/windsurf/bin/language_server_linux_x64");
            if path.is_file() {
                return Some(path);
            }
        }
    }

    let bin_root = config.state_dir.parent().map(|path| path.join("bin"))?;
    let mut candidates: Vec<_> = fs::read_dir(bin_root)
        .ok()?
        .flatten()
        .map(|entry| {
            entry
                .path()
                .join("extensions/windsurf/bin/language_server_linux_x64")
        })
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
) -> Result<SpawnedRuntime> {
    let language_server = find_language_server_binary(config).ok_or_else(|| {
        anyhow!(
            "headless language_server binary not found; set SURFWIND_LANGUAGE_SERVER_PATH to override"
        )
    })?;
    let database_dir = find_headless_database_dir().ok_or_else(|| {
        anyhow!("headless database directory not found; set SURFWIND_DATABASE_DIR to override")
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
        .env(MANAGED_RUNTIME_ENV_FLAG, "1")
        .env_remove("VSCODE_IPC_HOOK_CLI")
        .env_remove("VSCODE_CLIENT_COMMAND")
        .env_remove("VSCODE_CLIENT_COMMAND_CWD")
        .env_remove("VSCODE_CLI_AUTHORITY")
        .current_dir(workspace_root)
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file));

    let child = child.spawn().context("spawn headless language_server")?;
    thread::sleep(Duration::from_millis(500));
    Ok(SpawnedRuntime {
        pid: child.id() as i32,
        log_path,
    })
}

pub(crate) fn try_headless_auto_attach_runtime(
    config: &AppConfig,
    workspace_root: &Path,
) -> Result<()> {
    let requested_workspace_id = workspace_id_for_path(workspace_root);
    if let Some(record) = managed_runtime_record_for_workspace(config, &requested_workspace_id) {
        if pid_exists(record.pid) {
            let initial_deadline = Instant::now() + Duration::from_millis(500);
            if wait_for_workspace_runtime(config, &requested_workspace_id, initial_deadline) {
                return Ok(());
            }
            terminate_managed_runtime(record.pid);
            let _ = prune_stale_managed_runtime_records(config);
        }
    }
    let initial_deadline = Instant::now() + Duration::from_millis(200);
    if wait_for_workspace_runtime(config, &requested_workspace_id, initial_deadline) {
        return Ok(());
    }

    let spawned = spawn_headless_runtime(config, workspace_root, &requested_workspace_id)?;
    let now = now_iso();
    let _ = upsert_managed_runtime_record(
        config,
        ManagedRuntimeRecord {
            pid: spawned.pid,
            workspace_id: requested_workspace_id.clone(),
            workspace_root: workspace_root.display().to_string(),
            started_at: now.clone(),
            last_used_at: now,
        },
    );
    let poll_deadline =
        Instant::now() + Duration::from_secs_f64(config.auto_launch_timeout_sec.max(1.0));
    if wait_for_workspace_runtime(config, &requested_workspace_id, poll_deadline) {
        return Ok(());
    }

    terminate_managed_runtime(spawned.pid);
    let _ = prune_stale_managed_runtime_records(config);

    Err(anyhow!(
        "headless auto-attach did not attach workspace: {}; log tail: {}",
        workspace_root.display(),
        tail_log(&spawned.log_path)
    ))
}
