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
