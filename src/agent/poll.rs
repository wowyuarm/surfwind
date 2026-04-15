use anyhow::Result;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::AppConfig;
use crate::runstore::{get_run, list_runs, save_run, summarize_run};
use crate::runtime::{
    choose_active_port, discover_runtime, now_iso, rpc_call, sample_outbound_targets,
};
use crate::translator::{build_metadata, extract_assistant_text, extract_error_short};
use crate::types::{RunListItem, RunRecord};

pub fn list_agent_runs(config: &AppConfig, limit: usize) -> Result<Vec<RunListItem>> {
    list_agent_runs_filtered(config, limit, None, None)
}

pub fn list_agent_runs_filtered(
    config: &AppConfig,
    limit: usize,
    status: Option<&str>,
    workspace: Option<&str>,
) -> Result<Vec<RunListItem>> {
    let fetch_limit = if status.is_some() || workspace.is_some() {
        usize::MAX
    } else {
        limit.max(1)
    };
    Ok(list_runs(config, fetch_limit)?
        .into_iter()
        .map(|record| reconcile_and_store_run(config, record))
        .filter(|record| matches_run_status(record, status))
        .filter(|record| matches_run_workspace(record, workspace))
        .map(|record| summarize_run(&record))
        .take(limit.max(1))
        .collect())
}

pub fn get_agent_run(config: &AppConfig, run_id: &str) -> Result<Option<RunRecord>> {
    Ok(get_run(config, run_id)?.map(|record| reconcile_and_store_run(config, record)))
}

pub fn get_latest_agent_run(config: &AppConfig) -> Result<Option<RunRecord>> {
    Ok(list_runs(config, 1)?
        .into_iter()
        .next()
        .map(|record| reconcile_and_store_run(config, record)))
}

pub fn get_latest_resumable_agent_run(config: &AppConfig) -> Result<Option<RunRecord>> {
    Ok(list_runs(config, usize::MAX)?
        .into_iter()
        .map(|record| reconcile_and_store_run(config, record))
        .find(run_is_resumable))
}

fn run_is_resumable(record: &RunRecord) -> bool {
    record
        .cascade_id
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
}

fn matches_run_status(record: &RunRecord, status: Option<&str>) -> bool {
    let Some(status) = status.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    record.status.eq_ignore_ascii_case(status)
}

fn matches_run_workspace(record: &RunRecord, workspace: Option<&str>) -> bool {
    let Some(requested) = workspace.map(normalize_workspace_query) else {
        return true;
    };
    let Some(summary) = record.summary.as_object() else {
        return false;
    };
    summary
        .get("requestedWorkspace")
        .and_then(Value::as_str)
        .map(normalize_workspace_query)
        .as_ref()
        == Some(&requested)
        || summary
            .get("workspaceId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            == Some(requested.as_str())
}

fn normalize_workspace_query(raw: &str) -> String {
    let trimmed = raw.trim();
    let candidate = PathBuf::from(trimmed);
    candidate
        .canonicalize()
        .unwrap_or(candidate)
        .to_string_lossy()
        .to_string()
}

pub(crate) fn reconcile_and_store_run(config: &AppConfig, record: RunRecord) -> RunRecord {
    let refreshed = refresh_run_record(config, &record).unwrap_or_else(|_| record.clone());
    if refreshed.status != record.status
        || refreshed.upstream_status != record.upstream_status
        || refreshed.error != record.error
        || refreshed.output_text != record.output_text
        || refreshed.step_count != record.step_count
        || refreshed.events != record.events
    {
        let _ = save_run(config, &refreshed);
    }
    refreshed
}

fn refresh_run_record(config: &AppConfig, record: &RunRecord) -> Result<RunRecord> {
    if record.status != "running" {
        return Ok(record.clone());
    }
    let Some(cascade_id) = record.cascade_id.as_deref() else {
        return Ok(record.clone());
    };
    let requested_workspace = record
        .summary
        .get("requestedWorkspace")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let runtime = match discover_runtime(config, requested_workspace.as_deref(), false) {
        Ok(runtime) => runtime,
        Err(_) => return Ok(record.clone()),
    };
    let session_id = format!("surfwind-refresh-{}", Uuid::new_v4());
    let metadata = build_metadata(config, &runtime.api_key, &session_id);
    let Some(active_port) = choose_active_port(config, &runtime.ports, &runtime.csrf, &metadata)
    else {
        return Ok(record.clone());
    };
    let mut latest_steps = Vec::new();
    let mut assistant_text = None;
    let mut error_short = None;
    let mut final_status = None;

    let trajectory_res = rpc_call(
        config,
        active_port,
        &runtime.csrf,
        "GetCascadeTrajectory",
        &json!({ "cascadeId": cascade_id }),
    );
    if trajectory_res.status == 200 {
        let payload = safe_json_object(&trajectory_res.text);
        final_status = payload
            .get("status")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        latest_steps = slice_steps(
            payload
                .get("trajectory")
                .and_then(|value| value.get("steps"))
                .and_then(Value::as_array),
            record.step_offset,
        );
        assistant_text =
            prefer_assistant_text(assistant_text, extract_assistant_text(&latest_steps));
        error_short = error_short.or_else(|| extract_error_short(&latest_steps));
    }

    let settled = settle_terminal_status(
        config,
        active_port,
        &runtime.csrf,
        cascade_id,
        assistant_text,
        error_short,
        final_status,
        record.step_offset,
    );
    let mut assistant_text = settled.0;
    let mut error_short = settled.1;
    let final_status = settled.2;

    let final_steps = rpc_call(
        config,
        active_port,
        &runtime.csrf,
        "GetCascadeTrajectorySteps",
        &json!({ "cascadeId": cascade_id, "stepOffset": record.step_offset }),
    );
    if final_steps.status == 200 {
        let payload = safe_json_object(&final_steps.text);
        latest_steps = payload
            .get("steps")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assistant_text =
            prefer_assistant_text(assistant_text, extract_assistant_text(&latest_steps));
        error_short = error_short.or_else(|| extract_error_short(&latest_steps));
    }

    if let Some(workspace_root) = requested_workspace.as_deref() {
        if let Some(escaped_path) = detect_workspace_escape(&latest_steps, workspace_root) {
            error_short = Some(format!("workspace_fence_violation: {}", escaped_path));
        }
    }

    let completion_status = derive_completion_status(
        assistant_text.as_deref(),
        error_short.as_deref(),
        final_status.as_deref(),
    );
    let mut tool_calls = extract_tool_calls_from_steps(&latest_steps);

    let mut summary = record.summary.clone();
    summary["stage"] = json!("refresh_trajectory");
    summary["activePort"] = json!(active_port);
    summary["runtimePid"] = json!(runtime.pid);
    summary["candidatePorts"] = json!(runtime.ports.clone());
    summary["workspaceId"] = json!(runtime.workspace_id.clone());
    summary["upstreamStatus"] = json!(final_status.clone());
    summary["finalStatus"] = json!(completion_status.clone());
    summary["error"] = json!(error_short.clone());
    summary["outboundTargetsEnd"] = json!(sample_outbound_targets(runtime.pid));
    if let Some(text) = assistant_text.as_ref() {
        summary["assistantTextLength"] = json!(text.len());
    }
    if !tool_calls.is_empty() {
        summary["toolCallCount"] = json!(tool_calls.len());
    }
    if let Some(workspace_root) = requested_workspace.as_deref() {
        summary["workspaceFenceRoot"] = json!(workspace_root);
        if let Some(error) = error_short
            .as_ref()
            .filter(|value| value.starts_with("workspace_fence_violation:"))
        {
            summary["workspaceFenceViolation"] = json!(error);
        }
    }

    let mut events = strip_dynamic_events(&record.events);
    events.extend(build_step_events(&latest_steps, record.step_offset));
    if let Some(text) = assistant_text.as_ref() {
        events.push(event(
            "assistant.output",
            json!({ "chars": text.len(), "preview": truncate(text, 500) }),
        ));
    }
    if !tool_calls.is_empty() {
        events.push(event("tool.calls", json!({ "count": tool_calls.len() })));
    }
    if let Some(error) = error_short.as_ref() {
        events.push(event(
            "run.failed",
            json!({ "error": error, "finalStatus": completion_status }),
        ));
    } else if is_running_status(final_status.as_deref()) {
        events.push(event(
            "run.running",
            json!({
                "finalStatus": completion_status,
                "outputChars": assistant_text.as_ref().map(|text| text.len()).unwrap_or(0),
            }),
        ));
    } else {
        events.push(event(
            "run.completed",
            json!({
                "finalStatus": completion_status,
                "outputChars": assistant_text.as_ref().map(|text| text.len()).unwrap_or(0),
            }),
        ));
    }

    let status_code = if error_short.is_some() {
        502
    } else if is_running_status(final_status.as_deref()) {
        202
    } else {
        200
    };

    Ok(build_run_record(
        &record.run_id,
        &record.mode,
        &record.path,
        &record.prompt,
        record.request_model.as_deref(),
        &record.requested_model_uid,
        record.cascade_id.as_deref(),
        record.parent_run_id.clone(),
        status_code,
        assistant_text,
        std::mem::take(&mut tool_calls),
        error_short,
        Some(completion_status),
        summary,
        events,
        record.step_offset,
        latest_steps.len(),
        &record.created_at,
    ))
}

fn build_run_record(
    run_id: &str,
    mode: &str,
    path: &str,
    prompt: &str,
    model: Option<&str>,
    requested_model_uid: &str,
    cascade_id: Option<&str>,
    parent_run_id: Option<String>,
    http_status: u16,
    output_text: Option<String>,
    tool_calls: Vec<crate::types::ToolCallEnvelope>,
    error_text: Option<String>,
    final_status: Option<String>,
    summary: Value,
    events: Vec<Value>,
    step_offset: usize,
    new_step_count: usize,
    created_at: &str,
) -> RunRecord {
    RunRecord {
        run_id: run_id.to_string(),
        mode: mode.to_string(),
        path: path.to_string(),
        parent_run_id,
        prompt: prompt.to_string(),
        request_model: model.map(ToOwned::to_owned),
        requested_model_uid: requested_model_uid.to_string(),
        cascade_id: cascade_id.map(ToOwned::to_owned),
        status: status_label(http_status, output_text.as_deref(), error_text.as_deref()),
        http_status,
        upstream_status: final_status,
        error: error_text,
        output_text,
        tool_calls,
        step_offset,
        new_step_count,
        step_count: step_offset + new_step_count,
        created_at: created_at.to_string(),
        updated_at: now_iso(),
        completed_at: if http_status == 202 {
            None
        } else {
            Some(now_iso())
        },
        summary,
        events,
    }
}

fn status_label(http_status: u16, output_text: Option<&str>, error_text: Option<&str>) -> String {
    if http_status == 202 {
        "running".to_string()
    } else if http_status == 200 && (output_text.is_some() || error_text.is_none()) {
        "completed".to_string()
    } else if http_status == 400 {
        "invalid_request".to_string()
    } else {
        "failed".to_string()
    }
}

fn event(event_type: &str, data: Value) -> Value {
    json!({
        "type": event_type,
        "ts": now_iso(),
        "data": data,
    })
}

fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn safe_json_object(text: &str) -> Value {
    serde_json::from_str::<Value>(text)
        .ok()
        .filter(|value| value.is_object())
        .unwrap_or_else(|| json!({}))
}

fn slice_steps(steps: Option<&Vec<Value>>, step_offset: usize) -> Vec<Value> {
    let Some(steps) = steps else {
        return Vec::new();
    };
    if step_offset == 0 {
        return steps.clone();
    }
    steps.iter().skip(step_offset).cloned().collect()
}

fn prefer_assistant_text(current: Option<String>, candidate: Option<String>) -> Option<String> {
    match (current, candidate) {
        (None, candidate) => candidate.filter(|text| !text.trim().is_empty()),
        (Some(current), None) => Some(current),
        (Some(current), Some(candidate)) => {
            if candidate.trim().is_empty() {
                Some(current)
            } else if candidate.len() >= current.len() {
                Some(candidate)
            } else {
                Some(current)
            }
        }
    }
}

fn is_terminal_status(status: Option<&str>) -> bool {
    matches!(status, Some(value) if !value.is_empty() && value != "CASCADE_RUN_STATUS_RUNNING")
}

fn is_running_status(status: Option<&str>) -> bool {
    matches!(status, Some("CASCADE_RUN_STATUS_RUNNING"))
}

fn settle_terminal_status(
    config: &AppConfig,
    active_port: u16,
    csrf: &str,
    cascade_id: &str,
    assistant_text: Option<String>,
    error_short: Option<String>,
    final_status: Option<String>,
    step_offset: usize,
) -> (Option<String>, Option<String>, Option<String>) {
    if error_short.is_some() {
        return (assistant_text, error_short, final_status);
    }
    if assistant_text.is_some() && is_terminal_status(final_status.as_deref()) {
        return (assistant_text, error_short, final_status);
    }

    let mut assistant_text = assistant_text;
    let mut error_short = error_short;
    let mut final_status = final_status;
    let grace_sleep_ms = config.poll_interval_ms.min(800);

    for round in 0..5 {
        let steps_res = rpc_call(
            config,
            active_port,
            csrf,
            "GetCascadeTrajectorySteps",
            &json!({ "cascadeId": cascade_id, "stepOffset": step_offset }),
        );
        if steps_res.status == 200 {
            let payload = safe_json_object(&steps_res.text);
            let steps = payload
                .get("steps")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assistant_text = prefer_assistant_text(assistant_text, extract_assistant_text(&steps));
            error_short = error_short.or_else(|| extract_error_short(&steps));
        }

        let trajectory_res = rpc_call(
            config,
            active_port,
            csrf,
            "GetCascadeTrajectory",
            &json!({ "cascadeId": cascade_id }),
        );
        if trajectory_res.status == 200 {
            let payload = safe_json_object(&trajectory_res.text);
            final_status = payload
                .get("status")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let scoped_steps = slice_steps(
                payload
                    .get("trajectory")
                    .and_then(|value| value.get("steps"))
                    .and_then(Value::as_array),
                step_offset,
            );
            assistant_text =
                prefer_assistant_text(assistant_text, extract_assistant_text(&scoped_steps));
            error_short = error_short.or_else(|| extract_error_short(&scoped_steps));
            if assistant_text.is_some() && is_terminal_status(final_status.as_deref()) {
                break;
            }
        }
        if error_short.is_some() {
            break;
        }
        if round >= 4 && is_terminal_status(final_status.as_deref()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(grace_sleep_ms));
    }

    (assistant_text, error_short, final_status)
}

fn derive_completion_status(
    assistant_text: Option<&str>,
    error_short: Option<&str>,
    upstream_status: Option<&str>,
) -> String {
    if assistant_text.is_some()
        && error_short.is_none()
        && matches!(
            upstream_status,
            None | Some("") | Some("CASCADE_RUN_STATUS_RUNNING")
        )
    {
        "ASSISTANT_READY".to_string()
    } else {
        upstream_status.unwrap_or("unknown").to_string()
    }
}

fn build_step_events(steps: &[Value], step_offset: usize) -> Vec<Value> {
    let mut events = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        let mut data = json!({
            "stepIndex": step_offset + index,
            "stepType": step.get("type").and_then(Value::as_str).unwrap_or("unknown"),
        });
        let finish = step.get("finish").and_then(Value::as_object);
        let planner = step.get("plannerResponse").and_then(Value::as_object);
        let output = finish
            .and_then(|item| item.get("outputString"))
            .and_then(Value::as_str)
            .or_else(|| {
                planner
                    .and_then(|item| {
                        item.get("modifiedResponse")
                            .or_else(|| item.get("response"))
                    })
                    .and_then(Value::as_str)
            });
        if let Some(output) = output.filter(|text| !text.trim().is_empty()) {
            data["outputPreview"] = json!(truncate(output.trim(), 500));
        }
        let short_error = step
            .get("errorMessage")
            .and_then(Value::as_object)
            .and_then(|value| value.get("error"))
            .and_then(Value::as_object)
            .and_then(|value| value.get("shortError"))
            .and_then(Value::as_str);
        if let Some(short_error) = short_error.filter(|text| !text.trim().is_empty()) {
            data["error"] = json!(short_error.trim());
        }
        events.push(event("trajectory.step", data));
    }
    events
}

fn extract_tool_calls_from_steps(steps: &[Value]) -> Vec<crate::types::ToolCallEnvelope> {
    use crate::types::{ToolCallEnvelope, ToolFunction};

    let mut tool_calls = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        let step_type = step.get("type").and_then(Value::as_str);
        let tool_name = match step_type {
            Some("CORTEX_STEP_TYPE_VIEW_FILE") => "view_file",
            Some("CORTEX_STEP_TYPE_LIST_DIRECTORY") => "list_directory",
            Some("CORTEX_STEP_TYPE_EDIT_FILE") => "edit_file",
            Some("CORTEX_STEP_TYPE_CREATE_FILE") => "create_file",
            Some("CORTEX_STEP_TYPE_DELETE_FILE") => "delete_file",
            Some("CORTEX_STEP_TYPE_SHELL") => "shell",
            Some("CORTEX_STEP_TYPE_GREP_SEARCH") => "grep_search",
            Some("CORTEX_STEP_TYPE_RUN_COMMAND") => "run_command",
            _ => continue,
        };

        let arguments = if let Some(data) = step.get("data") {
            serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string())
        } else {
            serde_json::to_string(step).unwrap_or_else(|_| "{}".to_string())
        };

        let uuid_str = Uuid::new_v4().simple().to_string();
        let uuid_short = &uuid_str[..8.min(uuid_str.len())];

        tool_calls.push(ToolCallEnvelope {
            id: format!("call_{}_{}", index, uuid_short),
            kind: "function".to_string(),
            function: ToolFunction {
                name: tool_name.to_string(),
                arguments,
            },
        });
    }
    tool_calls
}

fn strip_dynamic_events(events: &[Value]) -> Vec<Value> {
    events
        .iter()
        .filter(|event| {
            let event_type = event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            !matches!(
                event_type,
                "trajectory.step"
                    | "assistant.output"
                    | "tool.calls"
                    | "run.failed"
                    | "run.running"
                    | "run.completed"
            )
        })
        .cloned()
        .collect()
}

fn detect_workspace_escape(steps: &[Value], workspace_root: &str) -> Option<String> {
    let root = normalize_workspace_root(workspace_root)?;
    for path in steps.iter().flat_map(extract_step_paths) {
        if !path.starts_with(&root) {
            return Some(path.display().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{get_latest_agent_run, get_latest_resumable_agent_run, list_agent_runs_filtered};
    use crate::runstore::save_run;
    use crate::settings::{SettingsData, SettingsPaths};
    use crate::types::RunRecord;
    use serde_json::json;
    use tempfile::TempDir;

    fn create_test_config(temp_dir: &TempDir) -> crate::config::AppConfig {
        let home = temp_dir.path().join(".surfwind");
        crate::config::AppConfig {
            paths: SettingsPaths {
                home_dir: home.clone(),
                settings_path: home.join("settings.json"),
                runs_dir: home.join("runs"),
                logs_dir: home.join("logs"),
                managed_runtimes_path: home.join("managed-runtimes.json"),
            },
            settings: SettingsData {
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

    fn create_test_run(run_id: &str, status: &str, workspace: &str) -> RunRecord {
        RunRecord {
            run_id: run_id.to_string(),
            mode: "agent".to_string(),
            path: "/test".to_string(),
            parent_run_id: None,
            prompt: "test prompt".to_string(),
            request_model: None,
            requested_model_uid: "test-model".to_string(),
            cascade_id: None,
            status: status.to_string(),
            http_status: if status == "running" { 202 } else { 200 },
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
            summary: json!({
                "requestedWorkspace": workspace,
                "workspaceId": format!("ws_{}", run_id),
            }),
            events: vec![],
        }
    }

    #[test]
    fn returns_latest_persisted_run() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);

        save_run(&config, &create_test_run("run-1", "completed", "/repo/a")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        save_run(&config, &create_test_run("run-2", "failed", "/repo/b")).unwrap();

        let latest = get_latest_agent_run(&config).unwrap().unwrap();
        assert_eq!(latest.run_id, "run-2");
    }

    #[test]
    fn returns_latest_resumable_run_skipping_non_resumable_latest() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);

        let mut resumable = create_test_run("run-1", "completed", "/repo/a");
        resumable.cascade_id = Some("cascade-1".to_string());
        let latest_non_resumable = create_test_run("run-2", "failed", "/repo/b");

        save_run(&config, &resumable).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        save_run(&config, &latest_non_resumable).unwrap();

        let latest = get_latest_agent_run(&config).unwrap().unwrap();
        let latest_resumable = get_latest_resumable_agent_run(&config).unwrap().unwrap();

        assert_eq!(latest.run_id, "run-2");
        assert_eq!(latest_resumable.run_id, "run-1");
    }

    #[test]
    fn filters_runs_by_status() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);

        save_run(&config, &create_test_run("run-1", "completed", "/repo/a")).unwrap();
        save_run(&config, &create_test_run("run-2", "failed", "/repo/b")).unwrap();

        let runs = list_agent_runs_filtered(&config, 20, Some("failed"), None).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, "run-2");
    }

    #[test]
    fn filters_runs_by_workspace() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        let workspace_a = temp_dir.path().join("repo-a");
        let workspace_b = temp_dir.path().join("repo-b");
        std::fs::create_dir_all(&workspace_a).unwrap();
        std::fs::create_dir_all(&workspace_b).unwrap();

        save_run(
            &config,
            &create_test_run("run-1", "completed", &workspace_a.to_string_lossy()),
        )
        .unwrap();
        save_run(
            &config,
            &create_test_run("run-2", "failed", &workspace_b.to_string_lossy()),
        )
        .unwrap();

        let runs =
            list_agent_runs_filtered(&config, 20, None, Some(&workspace_b.to_string_lossy()))
                .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, "run-2");
    }
}

fn normalize_workspace_root(workspace_root: &str) -> Option<PathBuf> {
    let path = PathBuf::from(workspace_root);
    if path.exists() {
        path.canonicalize().ok().or(Some(path))
    } else if path.is_absolute() {
        Some(path)
    } else {
        None
    }
}

fn extract_step_paths(step: &Value) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for key in [
        "listDirectory",
        "list_directory",
        "viewFile",
        "view_file",
        "readFile",
        "read_file",
    ] {
        if let Some(object) = step.get(key).and_then(Value::as_object) {
            push_paths_from_object(&mut paths, object);
        }
    }
    if let Some(error_message) = step
        .get("errorMessage")
        .and_then(Value::as_object)
        .and_then(|value| value.get("error"))
        .and_then(Value::as_object)
    {
        if let Some(details) = error_message.get("details").and_then(Value::as_str) {
            if let Ok(payload) = serde_json::from_str::<Value>(details) {
                if let Some(arguments_json) = payload.get("argumentsJson").and_then(Value::as_str) {
                    if let Ok(arguments) = serde_json::from_str::<Value>(arguments_json) {
                        if let Some(object) = arguments.as_object() {
                            push_paths_from_object(&mut paths, object);
                        }
                    }
                }
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

fn push_paths_from_object(paths: &mut Vec<PathBuf>, object: &serde_json::Map<String, Value>) {
    for key in [
        "directoryPathUri",
        "directory_path_uri",
        "directoryPath",
        "directory_path",
        "filePathUri",
        "file_path_uri",
        "file_path",
        "filePath",
        "uri",
        "path",
        "file_path",
    ] {
        if let Some(raw) = object.get(key).and_then(Value::as_str) {
            if let Some(path) = normalize_step_path(raw) {
                paths.push(path);
            }
        }
    }
}

fn normalize_step_path(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(path) = trimmed.strip_prefix("file://") {
        let candidate = PathBuf::from(path);
        if candidate.is_absolute() {
            return candidate.canonicalize().ok().or(Some(candidate));
        }
    }
    let candidate = Path::new(trimmed);
    if candidate.is_absolute() {
        let path = candidate.to_path_buf();
        return path.canonicalize().ok().or(Some(path));
    }
    None
}
