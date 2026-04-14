use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::AppConfig;
use crate::types::{RunListItem, RunRecord};

fn store_dir(config: &AppConfig) -> Result<PathBuf> {
    let path = config.run_store_dir();
    fs::create_dir_all(&path).with_context(|| format!("create {}", path.display()))?;
    Ok(path)
}

fn sanitize_run_id(run_id: &str) -> String {
    run_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .collect()
}

fn run_file(config: &AppConfig, run_id: &str) -> Result<PathBuf> {
    Ok(store_dir(config)?.join(format!("{}.json", sanitize_run_id(run_id))))
}

fn read_run_file(path: &Path) -> Option<RunRecord> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str::<RunRecord>(&text).ok()
}

fn prune(config: &AppConfig, max_runs: usize) -> Result<()> {
    let mut files: Vec<_> = fs::read_dir(store_dir(config)?)?
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect();
    files.sort_by_key(|entry| entry.metadata().and_then(|meta| meta.modified()).ok());
    files.reverse();
    for stale in files.into_iter().skip(max_runs) {
        let _ = fs::remove_file(stale.path());
    }
    Ok(())
}

pub fn save_run(config: &AppConfig, record: &RunRecord) -> Result<()> {
    let path = run_file(config, &record.run_id)?;
    let tmp = path.with_extension("json.tmp");
    let data = serde_json::to_string_pretty(record)?;
    fs::write(&tmp, data).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("rename {}", path.display()))?;
    prune(config, 200)?;
    Ok(())
}

pub fn get_run(config: &AppConfig, run_id: &str) -> Result<Option<RunRecord>> {
    let path = run_file(config, run_id)?;
    if !path.exists() {
        return Ok(None);
    }
    Ok(read_run_file(&path))
}

pub fn list_runs(config: &AppConfig, limit: usize) -> Result<Vec<RunRecord>> {
    let mut files: Vec<_> = fs::read_dir(store_dir(config)?)?
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect();
    files.sort_by_key(|entry| entry.metadata().and_then(|meta| meta.modified()).ok());
    files.reverse();

    let mut runs = Vec::new();
    for entry in files.into_iter().take(limit.max(1)) {
        if let Some(record) = read_run_file(&entry.path()) {
            runs.push(record);
        }
    }
    Ok(runs)
}

pub fn summarize_run(record: &RunRecord) -> RunListItem {
    let workspace_id = record
        .summary
        .get("workspaceId")
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned);
    let workspace = record
        .summary
        .get("requestedWorkspace")
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned);

    RunListItem {
        run_id: record.run_id.clone(),
        mode: record.mode.clone(),
        status: record.status.clone(),
        parent_run_id: record.parent_run_id.clone(),
        requested_model_uid: record.requested_model_uid.clone(),
        cascade_id: record.cascade_id.clone(),
        workspace_id,
        workspace,
        created_at: record.created_at.clone(),
        updated_at: record.updated_at.clone(),
        error: record.error.clone(),
        output_preview: record
            .output_text
            .clone()
            .unwrap_or_default()
            .chars()
            .take(200)
            .collect(),
        step_count: record.step_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RunRecord;
    use serde_json::json;
    use tempfile::TempDir;

    fn create_test_config(temp_dir: &TempDir) -> AppConfig {
        let home = temp_dir.path().join(".surfwind");
        AppConfig {
            paths: crate::settings::SettingsPaths {
                home_dir: home.clone(),
                settings_path: home.join("settings.json"),
                runs_dir: home.join("runs"),
                logs_dir: home.join("logs"),
                managed_runtimes_path: home.join("managed-runtimes.json"),
            },
            settings: crate::settings::SettingsData {
                model: "test-model".to_string(),
                run_store_dir: temp_dir.path().join("runs").to_string_lossy().to_string(),
                output: "text".to_string(),
            },
            state_dir: temp_dir.path().join("state").to_path_buf(),
            user_settings_path: temp_dir.path().join("user_settings.pb").to_path_buf(),
            metadata_api_key: None,
            rpc_timeout_sec: 20.0,
            poll_interval_ms: 800,
            poll_max_rounds: 45,
            auto_launch_enabled: false,
            auto_launch_timeout_sec: 15.0,
            auto_launch_poll_interval_ms: 500,
            metadata_ide_name: "test".to_string(),
            metadata_ide_version: "1.0.0".to_string(),
            metadata_extension_name: "test".to_string(),
            metadata_extension_version: "1.0.0".to_string(),
            metadata_locale: "en".to_string(),
            metadata_os: "linux".to_string(),
        }
    }

    fn create_test_run(run_id: &str) -> RunRecord {
        RunRecord {
            run_id: run_id.to_string(),
            mode: "agent".to_string(),
            path: "/test".to_string(),
            parent_run_id: None,
            prompt: "test prompt".to_string(),
            request_model: None,
            requested_model_uid: "test-model".to_string(),
            cascade_id: Some("test-cascade".to_string()),
            status: "completed".to_string(),
            http_status: 200,
            upstream_status: None,
            error: None,
            output_text: Some("test output".to_string()),
            tool_calls: vec![],
            step_offset: 0,
            new_step_count: 1,
            step_count: 1,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:01Z".to_string(),
            completed_at: Some("2024-01-01T00:00:02Z".to_string()),
            summary: json!({}),
            events: vec![],
        }
    }

    #[test]
    fn test_sanitize_run_id() {
        assert_eq!(sanitize_run_id("abc123"), "abc123");
        assert_eq!(sanitize_run_id("test-run_id-123"), "test-run_id-123");
        assert_eq!(sanitize_run_id("test.run!@#id"), "testrunid");
        assert_eq!(sanitize_run_id("run/with/slashes"), "runwithslashes");
        assert_eq!(sanitize_run_id(""), "");
    }

    #[test]
    fn test_save_and_get_run() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);

        let run = create_test_run("test-run-1");
        save_run(&config, &run).unwrap();

        let retrieved = get_run(&config, "test-run-1").unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.run_id, "test-run-1");
        assert_eq!(retrieved.output_text, Some("test output".to_string()));
    }

    #[test]
    fn test_get_nonexistent_run() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);

        let result = get_run(&config, "nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_list_runs() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);

        let run1 = create_test_run("run-1");
        let run2 = create_test_run("run-2");
        save_run(&config, &run1).unwrap();
        save_run(&config, &run2).unwrap();

        let runs = list_runs(&config, 10).unwrap();
        assert_eq!(runs.len(), 2);
    }

    #[test]
    fn test_summarize_run() {
        let mut run = create_test_run("test-run");
        run.output_text = Some("This is a long output text that should be truncated".to_string());
        run.summary = json!({
            "workspaceId": "ws-123",
            "requestedWorkspace": "/home/test"
        });

        let summary = summarize_run(&run);
        assert_eq!(summary.run_id, "test-run");
        assert_eq!(summary.workspace_id, Some("ws-123".to_string()));
        assert_eq!(summary.workspace, Some("/home/test".to_string()));
        assert!(summary.output_preview.len() <= 200);
    }
}
