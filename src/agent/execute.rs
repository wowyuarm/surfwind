use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::poll::reconcile_and_store_run;
use crate::config::AppConfig;
use crate::runstore::{get_run, save_run};
use crate::runtime::{
    choose_active_port, discover_runtime, now_iso, resolve_workspace_root, rpc_call,
    sample_outbound_targets,
};
use crate::translator::{build_metadata, extract_assistant_text, extract_error_short};
use crate::types::{AgentRunResult, RunRecord};

#[derive(Clone, Copy, Debug)]
pub struct AgentRunOptions {
    pub persist: bool,
    pub auto_launch: bool,
}

impl Default for AgentRunOptions {
    fn default() -> Self {
        Self {
            persist: true,
            auto_launch: true,
        }
    }
}

pub fn execute_agent_prompt(
    config: &AppConfig,
    prompt: &str,
    model: Option<&str>,
    workspace: Option<&str>,
    options: AgentRunOptions,
) -> AgentRunResult {
    execute_run(
        config,
        prompt,
        model,
        workspace,
        options,
        "/v1/agent/exec",
        None,
        "exec",
        None,
        None,
        0,
    )
}

pub fn resume_agent_prompt(
    config: &AppConfig,
    parent_run_id: &str,
    prompt: &str,
    model: Option<&str>,
    workspace: Option<&str>,
    options: AgentRunOptions,
) -> AgentRunResult {
    let created_at = now_iso();
    let Some(parent) = get_run(config, parent_run_id).ok().flatten() else {
        let run_id = new_run_id();
        let mut record = simple_failed_run(
            run_id,
            "resume",
            "/v1/agent/runs/resume",
            Some(parent_run_id.to_string()),
            prompt,
            requested_model_uid(model, Some(&config.default_model_uid())),
            "parent_run_not_found",
            404,
            created_at,
        );
        apply_run_record_options(&mut record, config, options);
        store_run_if_enabled(config, options.persist, &record);
        return AgentRunResult {
            status: 404,
            body: json!({
                "error": { "message": "parent run not found", "code": "parent_run_not_found" },
                "run": record,
            }),
            run: record,
        };
    };
    let parent = reconcile_and_store_run(config, parent);
    if parent.status == "running" {
        let run_id = new_run_id();
        let mut record = simple_failed_run(
            run_id,
            "resume",
            "/v1/agent/runs/resume",
            Some(parent_run_id.to_string()),
            prompt,
            requested_model_uid(model, Some(&parent.requested_model_uid)),
            "parent_run_still_running",
            409,
            created_at,
        );
        apply_run_record_options(&mut record, config, options);
        store_run_if_enabled(config, options.persist, &record);
        return AgentRunResult {
            status: 409,
            body: json!({
                "error": { "message": "parent run is still running", "code": "parent_run_still_running" },
                "run": record,
            }),
            run: record,
        };
    }

    let Some(cascade_id) = parent.cascade_id.clone() else {
        let run_id = new_run_id();
        let mut record = simple_failed_run(
            run_id,
            "resume",
            "/v1/agent/runs/resume",
            Some(parent_run_id.to_string()),
            prompt,
            requested_model_uid(model, Some(&parent.requested_model_uid)),
            "parent_run_missing_cascade",
            400,
            created_at,
        );
        apply_run_record_options(&mut record, config, options);
        store_run_if_enabled(config, options.persist, &record);
        return AgentRunResult {
            status: 400,
            body: json!({
                "error": { "message": "parent run has no cascadeId", "code": "parent_run_missing_cascade" },
                "run": record,
            }),
            run: record,
        };
    };

    let workspace = workspace.map(str::to_string).or_else(|| {
        parent
            .summary
            .get("requestedWorkspace")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    });

    execute_run(
        config,
        prompt,
        model.or(Some(parent.requested_model_uid.as_str())),
        workspace.as_deref(),
        options,
        "/v1/agent/runs/resume",
        None,
        "resume",
        Some(parent_run_id.to_string()),
        Some(cascade_id),
        parent.step_count,
    )
}

pub fn execute_run(
    config: &AppConfig,
    prompt: &str,
    model: Option<&str>,
    workspace: Option<&str>,
    options: AgentRunOptions,
    path: &str,
    run_id: Option<String>,
    mode: &str,
    parent_run_id: Option<String>,
    existing_cascade_id: Option<String>,
    step_offset: usize,
) -> AgentRunResult {
    let effective_run_id = run_id.unwrap_or_else(new_run_id);
    let created_at = now_iso();
    let requested_model_uid = requested_model_uid(model, Some(&config.default_model_uid()));
    let mut summary = json!({
        "runId": effective_run_id,
        "path": path,
        "startedAt": created_at,
        "stage": "validate_request",
        "mode": mode,
        "persisted": options.persist,
        "autoLaunchRequested": options.auto_launch,
        "autoLaunchEffective": options.auto_launch && config.auto_launch_enabled,
    });
    let mut events = vec![event(
        "run.created",
        json!({
            "runId": effective_run_id,
            "mode": mode,
            "parentRunId": parent_run_id,
        }),
    )];
    events.push(event(
        "run.options",
        agent_run_options_payload(config, options),
    ));

    if prompt.trim().is_empty() {
        summary["error"] = json!("prompt_required");
        events.push(event("run.failed", json!({ "error": "prompt_required" })));
        let record = build_run_record(
            &effective_run_id,
            mode,
            path,
            prompt,
            model,
            &requested_model_uid,
            existing_cascade_id.as_deref(),
            parent_run_id.clone(),
            400,
            None,
            Vec::new(),
            Some("prompt_required".to_string()),
            None,
            summary,
            events,
            step_offset,
            0,
            &created_at,
        );
        store_run_if_enabled(config, options.persist, &record);
        return AgentRunResult {
            status: 400,
            body: json!({
                "error": { "message": "prompt is required", "code": "prompt_required" },
                "run": record,
            }),
            run: record,
        };
    }

    let prompt = prompt.trim();
    summary["promptChars"] = json!(prompt.len());
    summary["requestedModelUid"] = json!(requested_model_uid.clone());

    let requested_workspace_root = match workspace.filter(|value| !value.trim().is_empty()) {
        Some(value) => match resolve_workspace_root(Some(value)) {
            Ok(pathbuf) => {
                let path = pathbuf.display().to_string();
                summary["requestedWorkspace"] = json!(path.clone());
                summary["autoLaunchEnabled"] = json!(config.auto_launch_enabled);
                Some(path)
            }
            Err(err) => {
                summary["stage"] = json!("validate_workspace");
                summary["error"] = json!("workspace_not_found");
                events.push(event(
                    "run.failed",
                    json!({ "error": "workspace_not_found", "workspace": value }),
                ));
                let record = build_run_record(
                    &effective_run_id,
                    mode,
                    path,
                    prompt,
                    model,
                    &requested_model_uid,
                    existing_cascade_id.as_deref(),
                    parent_run_id.clone(),
                    400,
                    None,
                    Vec::new(),
                    Some("workspace_not_found".to_string()),
                    None,
                    summary,
                    events,
                    step_offset,
                    0,
                    &created_at,
                );
                store_run_if_enabled(config, options.persist, &record);
                return AgentRunResult {
                    status: 400,
                    body: json!({
                        "error": { "message": err.to_string(), "code": "workspace_not_found" },
                        "run": record,
                    }),
                    run: record,
                };
            }
        },
        None => None,
    };
    if let Some(workspace_root) = requested_workspace_root.as_deref() {
        summary["workspaceFenceRoot"] = json!(workspace_root);
    }
    let effective_prompt = inject_workspace_fence(prompt, requested_workspace_root.as_deref());
    if effective_prompt != prompt {
        summary["effectivePromptChars"] = json!(effective_prompt.len());
    }

    let runtime = match discover_runtime(config, requested_workspace_root.as_deref(), options.auto_launch) {
        Ok(runtime) => runtime,
        Err(err) => {
            let text = err.to_string();
            let attach_only = requested_workspace_root.is_some()
                && text.contains("no active language_server for workspace:");
            let safe_auto_attach_error = requested_workspace_root.is_some()
                && (text.contains("safe auto-attach") || text.contains("did not attach workspace"));
            let error_code = if safe_auto_attach_error {
                "workspace_auto_attach_failed"
            } else if attach_only {
                "workspace_not_attached"
            } else {
                "runtime_discovery_failed"
            };
            let status_code = if safe_auto_attach_error {
                502
            } else if attach_only {
                409
            } else {
                502
            };
            summary["stage"] = json!("discover_runtime");
            summary["error"] = json!(error_code);
            events.push(event(
                "run.failed",
                json!({ "error": error_code, "message": text }),
            ));
            let record = build_run_record(
                &effective_run_id,
                mode,
                path,
                prompt,
                model,
                &requested_model_uid,
                existing_cascade_id.as_deref(),
                parent_run_id.clone(),
                status_code,
                None,
                Vec::new(),
                Some(error_code.to_string()),
                None,
                summary,
                events,
                step_offset,
                0,
                &created_at,
            );
            store_run_if_enabled(config, options.persist, &record);
            return AgentRunResult {
                status: status_code,
                body: json!({
                    "error": { "message": text, "code": error_code },
                    "run": record,
                }),
                run: record,
            };
        }
    };

    let session_id = format!("surfwind-{}", Uuid::new_v4());
    let metadata = build_metadata(config, &runtime.api_key, &session_id);
    summary["sessionId"] = json!(session_id.clone());
    summary["runtimePid"] = json!(runtime.pid);
    summary["candidatePorts"] = json!(runtime.ports.clone());
    summary["workspaceId"] = json!(runtime.workspace_id.clone());

    let active_port = choose_active_port(config, &runtime.ports, &runtime.csrf, &metadata);
    summary["activePort"] = json!(active_port);
    events.push(event(
        "runtime.selected",
        json!({
            "pid": runtime.pid,
            "activePort": active_port,
            "candidatePorts": runtime.ports,
            "workspaceId": runtime.workspace_id,
            "requestedWorkspace": requested_workspace_root,
        }),
    ));

    let Some(active_port) = active_port else {
        summary["stage"] = json!("choose_active_port");
        summary["error"] = json!("no_active_port");
        events.push(event("run.failed", json!({ "error": "no_active_port" })));
        let record = build_run_record(
            &effective_run_id,
            mode,
            path,
            prompt,
            model,
            &requested_model_uid,
            existing_cascade_id.as_deref(),
            parent_run_id.clone(),
            502,
            None,
            Vec::new(),
            Some("no_active_port".to_string()),
            None,
            summary,
            events,
            step_offset,
            0,
            &created_at,
        );
        store_run_if_enabled(config, options.persist, &record);
        return AgentRunResult {
            status: 502,
            body: json!({
                "error": { "message": "no working language_server port", "code": "no_active_port" },
                "run": record,
            }),
            run: record,
        };
    };

    summary["outboundTargets"] = json!(sample_outbound_targets(runtime.pid));

    summary["stage"] = json!("workspace_trust");
    let trust = rpc_call(
        config,
        active_port,
        &runtime.csrf,
        "UpdateWorkspaceTrust",
        &json!({
            "metadata": metadata,
            "workspaceTrusted": true,
        }),
    );
    if trust.status != 200 {
        summary["error"] = json!("workspace_trust_failed");
        events.push(event(
            "run.failed",
            json!({ "error": "workspace_trust_failed" }),
        ));
        let record = build_run_record(
            &effective_run_id,
            mode,
            path,
            prompt,
            model,
            &requested_model_uid,
            existing_cascade_id.as_deref(),
            parent_run_id.clone(),
            502,
            None,
            Vec::new(),
            Some("workspace_trust_failed".to_string()),
            None,
            summary,
            events,
            step_offset,
            0,
            &created_at,
        );
        store_run_if_enabled(config, options.persist, &record);
        return AgentRunResult {
            status: 502,
            body: json!({
                "error": { "message": "UpdateWorkspaceTrust failed", "code": "workspace_trust_failed" },
                "run": record,
            }),
            run: record,
        };
    }

    let cascade_id = match existing_cascade_id {
        Some(cascade_id) => {
            events.push(event(
                "cascade.reused",
                json!({ "cascadeId": cascade_id, "parentRunId": parent_run_id }),
            ));
            cascade_id
        }
        None => {
            summary["stage"] = json!("start_cascade");
            let start = rpc_call(
                config,
                active_port,
                &runtime.csrf,
                "StartCascade",
                &json!({ "metadata": metadata }),
            );
            if start.status != 200 {
                summary["error"] = json!("start_failed");
                events.push(event("run.failed", json!({ "error": "start_failed" })));
                let record = build_run_record(
                    &effective_run_id,
                    mode,
                    path,
                    prompt,
                    model,
                    &requested_model_uid,
                    None,
                    parent_run_id.clone(),
                    502,
                    None,
                    Vec::new(),
                    Some("start_failed".to_string()),
                    None,
                    summary,
                    events,
                    step_offset,
                    0,
                    &created_at,
                );
                store_run_if_enabled(config, options.persist, &record);
                return AgentRunResult {
                    status: 502,
                    body: json!({
                        "error": { "message": "StartCascade failed", "code": "start_failed" },
                        "run": record,
                    }),
                    run: record,
                };
            }
            let payload = safe_json_object(&start.text);
            let Some(cascade_id) = payload
                .get("cascadeId")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
            else {
                summary["error"] = json!("invalid_start_response");
                events.push(event(
                    "run.failed",
                    json!({ "error": "invalid_start_response" }),
                ));
                let record = build_run_record(
                    &effective_run_id,
                    mode,
                    path,
                    prompt,
                    model,
                    &requested_model_uid,
                    None,
                    parent_run_id.clone(),
                    502,
                    None,
                    Vec::new(),
                    Some("invalid_start_response".to_string()),
                    None,
                    summary,
                    events,
                    step_offset,
                    0,
                    &created_at,
                );
                store_run_if_enabled(config, options.persist, &record);
                return AgentRunResult {
                    status: 502,
                    body: json!({
                        "error": { "message": "StartCascade response missing cascadeId", "code": "invalid_start_response" },
                        "run": record,
                    }),
                    run: record,
                };
            };
            events.push(event("cascade.started", json!({ "cascadeId": cascade_id })));
            cascade_id
        }
    };

    summary["cascadeId"] = json!(cascade_id.clone());
    summary["stage"] = json!("send_message");
    let send = rpc_call(
        config,
        active_port,
        &runtime.csrf,
        "SendUserCascadeMessage",
        &json!({
            "metadata": metadata,
            "cascadeId": cascade_id,
            "items": [{ "text": effective_prompt }],
            "cascadeConfig": {
                "plannerConfig": {
                    "conversational": {},
                    "requestedModelUid": requested_model_uid,
                }
            }
        }),
    );
    if send.status != 200 {
        summary["error"] = json!("send_failed");
        events.push(event("run.failed", json!({ "error": "send_failed" })));
        let record = build_run_record(
            &effective_run_id,
            mode,
            path,
            prompt,
            model,
            &requested_model_uid,
            Some(&cascade_id),
            parent_run_id.clone(),
            502,
            None,
            Vec::new(),
            Some("send_failed".to_string()),
            None,
            summary,
            events,
            step_offset,
            0,
            &created_at,
        );
        store_run_if_enabled(config, options.persist, &record);
        return AgentRunResult {
            status: 502,
            body: json!({
                "error": { "message": "SendUserCascadeMessage failed", "code": "send_failed" },
                "run": record,
            }),
            run: record,
        };
    }

    events.push(event(
        "message.sent",
        json!({ "cascadeId": cascade_id, "promptChars": effective_prompt.len() }),
    ));

    let mut assistant_text: Option<String> = None;
    let mut error_short: Option<String> = None;
    let mut final_status: Option<String> = None;
    let mut latest_steps: Vec<Value> = Vec::new();
    let mut post_terminal_rounds = 0usize;

    summary["stage"] = json!("poll_trajectory");
    for _ in 0..config.poll_max_rounds {
        let steps_res = rpc_call(
            config,
            active_port,
            &runtime.csrf,
            "GetCascadeTrajectorySteps",
            &json!({ "cascadeId": cascade_id, "stepOffset": step_offset }),
        );
        if steps_res.status == 200 {
            let payload = safe_json_object(&steps_res.text);
            let step_slice = payload
                .get("steps")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            latest_steps = step_slice;
            assistant_text =
                prefer_assistant_text(assistant_text, extract_assistant_text(&latest_steps));
            error_short = error_short.or_else(|| extract_error_short(&latest_steps));
        }

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
            let scoped_steps = slice_steps(
                payload
                    .get("trajectory")
                    .and_then(|value| value.get("steps"))
                    .and_then(Value::as_array),
                step_offset,
            );
            if !scoped_steps.is_empty() {
                latest_steps = scoped_steps;
                assistant_text =
                    prefer_assistant_text(assistant_text, extract_assistant_text(&latest_steps));
                error_short = error_short.or_else(|| extract_error_short(&latest_steps));
            }
        }

        if error_short.is_some() {
            break;
        }
        if assistant_text.is_some() && is_terminal_status(final_status.as_deref()) {
            break;
        }
        if is_terminal_status(final_status.as_deref()) {
            post_terminal_rounds += 1;
            if post_terminal_rounds >= 3 {
                break;
            }
        } else {
            post_terminal_rounds = 0;
        }
        std::thread::sleep(std::time::Duration::from_millis(config.poll_interval_ms));
    }

    let settled = settle_terminal_status(
        config,
        active_port,
        &runtime.csrf,
        &cascade_id,
        assistant_text.clone(),
        error_short.clone(),
        final_status.clone(),
        step_offset,
    );
    assistant_text = settled.0;
    error_short = settled.1;
    final_status = settled.2;

    let final_steps = rpc_call(
        config,
        active_port,
        &runtime.csrf,
        "GetCascadeTrajectorySteps",
        &json!({ "cascadeId": cascade_id, "stepOffset": step_offset }),
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
    if let Some(workspace_root) = requested_workspace_root.as_deref() {
        if let Some(escaped_path) = detect_workspace_escape(&latest_steps, workspace_root) {
            error_short = Some(format!("workspace_fence_violation: {}", escaped_path));
            summary["workspaceFenceViolation"] = json!(error_short.clone());
        }
    }

    let completion_status = derive_completion_status(
        assistant_text.as_deref(),
        error_short.as_deref(),
        final_status.as_deref(),
    );
    summary["upstreamStatus"] = json!(final_status.clone());
    summary["finalStatus"] = json!(completion_status.clone());
    summary["error"] = json!(error_short.clone());

    // Extract tool calls from trajectory steps (structured format)
    let mut tool_calls = extract_tool_calls_from_steps(&latest_steps);

    if !tool_calls.is_empty() {
        summary["toolCallCount"] = json!(tool_calls.len());
    }

    events.extend(build_step_events(&latest_steps, step_offset));
    if let Some(text) = assistant_text.as_ref() {
        summary["assistantTextLength"] = json!(text.len());
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

    summary["outboundTargetsEnd"] = json!(sample_outbound_targets(runtime.pid));
    let status_code = if error_short.is_some() {
        502
    } else if is_running_status(final_status.as_deref()) {
        202
    } else {
        200
    };
    let record = build_run_record(
        &effective_run_id,
        mode,
        path,
        prompt,
        model,
        &requested_model_uid,
        Some(&cascade_id),
        parent_run_id.clone(),
        status_code,
        assistant_text,
        std::mem::take(&mut tool_calls),
        error_short.clone(),
        Some(completion_status.clone()),
        summary,
        events,
        step_offset,
        latest_steps.len(),
        &created_at,
    );
    store_run_if_enabled(config, options.persist, &record);

    let mut body = json!({ "run": record.clone() });
    if let Some(error) = error_short {
        body["error"] = json!({ "message": error, "code": "trajectory_error" });
    }
    AgentRunResult {
        status: status_code,
        body,
        run: record,
    }
}

fn agent_run_options_payload(config: &AppConfig, options: AgentRunOptions) -> Value {
    json!({
        "persist": options.persist,
        "autoLaunchRequested": options.auto_launch,
        "autoLaunchEffective": options.auto_launch && config.auto_launch_enabled,
    })
}

fn apply_run_record_options(record: &mut RunRecord, config: &AppConfig, options: AgentRunOptions) {
    record.summary["persisted"] = json!(options.persist);
    record.summary["autoLaunchRequested"] = json!(options.auto_launch);
    record.summary["autoLaunchEffective"] = json!(options.auto_launch && config.auto_launch_enabled);
    if !record
        .events
        .iter()
        .any(|event| event.get("type").and_then(Value::as_str) == Some("run.options"))
    {
        let insert_at = record.events.len().min(1);
        record
            .events
            .insert(insert_at, event("run.options", agent_run_options_payload(config, options)));
    }
}

fn store_run_if_enabled(config: &AppConfig, persist: bool, record: &RunRecord) {
    if persist {
        let _ = save_run(config, record);
    }
}

fn requested_model_uid(model: Option<&str>, fallback: Option<&str>) -> String {
    model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| fallback.map(str::trim).filter(|value| !value.is_empty()))
        .unwrap_or("swe-1-6")
        .to_string()
}

fn new_run_id() -> String {
    format!("surf-run-{}", &Uuid::new_v4().simple().to_string()[..12])
}

fn simple_failed_run(
    run_id: String,
    mode: &str,
    path: &str,
    parent_run_id: Option<String>,
    prompt: &str,
    requested_model_uid: String,
    error: &str,
    http_status: u16,
    created_at: String,
) -> RunRecord {
    build_run_record(
        &run_id,
        mode,
        path,
        prompt,
        None,
        &requested_model_uid,
        None,
        parent_run_id,
        http_status,
        None,
        Vec::new(),
        Some(error.to_string()),
        None,
        json!({ "runId": run_id, "path": path, "error": error, "mode": mode }),
        vec![
            event("run.created", json!({ "runId": run_id, "mode": mode })),
            event("run.failed", json!({ "error": error })),
        ],
        0,
        0,
        &created_at,
    )
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
        let planner = step
            .get("plannerResponse")
            .and_then(Value::as_object);
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
        
        // Extract arguments from step data if available
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

fn inject_workspace_fence(prompt: &str, workspace_root: Option<&str>) -> String {
    let Some(workspace_root) = workspace_root else {
        return prompt.to_string();
    };
    format!(
        "{prompt}\n\n<workspace_fence>\nOnly inspect files under this workspace root: {workspace_root}\nUse absolute paths rooted at this workspace whenever you call file tools.\nDo not browse parent directories, sibling projects, or unrelated workspaces.\nIf the task seems ambiguous, ask for clarification instead of leaving this workspace.\n</workspace_fence>"
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_tool_calls_from_steps() {
        let steps = vec![
            serde_json::json!({"type": "CORTEX_STEP_TYPE_PLANNER_RESPONSE"}),
            serde_json::json!({"type": "CORTEX_STEP_TYPE_VIEW_FILE"}),
            serde_json::json!({"type": "CORTEX_STEP_TYPE_LIST_DIRECTORY"}),
            serde_json::json!({"type": "CORTEX_STEP_TYPE_FINISH"}),
        ];
        let calls = extract_tool_calls_from_steps(&steps);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "view_file");
        assert_eq!(calls[1].function.name, "list_directory");
    }

    #[test]
    fn test_extract_tool_calls_from_steps_empty() {
        let steps: Vec<serde_json::Value> = vec![];
        let calls = extract_tool_calls_from_steps(&steps);
        assert!(calls.is_empty());
    }

    #[test]
    fn test_extract_tool_calls_from_steps_non_tool_steps() {
        let steps = vec![
            serde_json::json!({"type": "CORTEX_STEP_TYPE_PLANNER_RESPONSE"}),
            serde_json::json!({"type": "CORTEX_STEP_TYPE_FINISH"}),
        ];
        let calls = extract_tool_calls_from_steps(&steps);
        assert!(calls.is_empty());
    }

    #[test]
    fn test_inject_workspace_fence() {
        let prompt = "Help me with code";
        let result = inject_workspace_fence(prompt, Some("/home/user/project"));
        assert!(result.contains("Help me with code"));
        assert!(result.contains("<workspace_fence>"));
        assert!(result.contains("/home/user/project"));
    }

    #[test]
    fn test_inject_workspace_fence_no_workspace() {
        let prompt = "Help me";
        let result = inject_workspace_fence(prompt, None);
        assert_eq!(result, "Help me".to_string());
    }

    #[test]
    fn test_requested_model_uid_with_value() {
        assert_eq!(requested_model_uid(Some("gpt-4"), Some("default")), "gpt-4");
    }

    #[test]
    fn test_requested_model_uid_with_empty() {
        assert_eq!(requested_model_uid(Some(""), Some("default")), "default");
    }

    #[test]
    fn test_requested_model_uid_with_none() {
        assert_eq!(requested_model_uid(None, Some("default")), "default");
    }

    #[test]
    fn test_requested_model_uid_no_fallback() {
        assert_eq!(requested_model_uid(None, None), "swe-1-6");
    }

    #[test]
    fn test_new_run_id_format() {
        let id = new_run_id();
        assert!(id.starts_with("surf-run-"));
        assert_eq!(id.len(), 21); // "surf-run-" (9) + 12 chars from uuid
    }

    #[test]
    fn test_status_label_202() {
        assert_eq!(status_label(202, Some("text"), None), "running");
    }

    #[test]
    fn test_status_label_200_with_output() {
        assert_eq!(status_label(200, Some("output"), None), "completed");
    }

    #[test]
    fn test_status_label_200_no_output() {
        assert_eq!(status_label(200, None, Some("error")), "failed");
    }

    #[test]
    fn test_status_label_error_status() {
        assert_eq!(status_label(500, None, Some("error")), "failed");
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello");
        assert_eq!(truncate("", 5), "");
    }

    #[test]
    fn test_is_terminal_status() {
        assert!(!is_terminal_status(None));
        assert!(!is_terminal_status(Some("")));
        assert!(!is_terminal_status(Some("CASCADE_RUN_STATUS_RUNNING")));
        assert!(is_terminal_status(Some("CASCADE_RUN_STATUS_COMPLETED")));
    }

    #[test]
    fn test_is_running_status() {
        assert!(!is_running_status(None));
        assert!(!is_running_status(Some("completed")));
        assert!(is_running_status(Some("CASCADE_RUN_STATUS_RUNNING")));
    }

    #[test]
    fn test_truncate_long_run_id() {
        let long_id = "a".repeat(100);
        assert_eq!(truncate(&long_id, 32).len(), 32);
    }

    #[test]
    fn test_safe_json_object_valid() {
        let json = r#"{"key": "value"}"#;
        let result = safe_json_object(json);
        assert!(result.is_object());
        assert_eq!(result["key"], "value");
    }

    #[test]
    fn test_safe_json_object_invalid() {
        let json = "not valid json";
        let result = safe_json_object(json);
        assert!(result.is_object());
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_safe_json_object_not_object() {
        let json = "123";
        let result = safe_json_object(json);
        assert!(result.is_object());
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_slice_steps() {
        let steps = vec![json!("step1"), json!("step2"), json!("step3")];
        let result = slice_steps(Some(&steps), 1);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], "step2");
    }

    #[test]
    fn test_slice_steps_none() {
        let result: Vec<Value> = slice_steps(None, 0);
        assert!(result.is_empty());
    }

    #[test]
    fn test_prefer_assistant_text() {
        assert_eq!(
            prefer_assistant_text(None, Some("new".to_string())),
            Some("new".to_string())
        );
        assert_eq!(
            prefer_assistant_text(Some("current".to_string()), None),
            Some("current".to_string())
        );
        assert_eq!(
            prefer_assistant_text(Some("current".to_string()), Some("new".to_string())),
            Some("current".to_string())
        );
    }

    #[test]
    fn test_event_structure() {
        let data = json!({"test": "data"});
        let evt = event("test.event", data.clone());
        assert_eq!(evt["type"], "test.event");
        assert!(evt.get("ts").is_some());
        assert_eq!(evt["data"], data);
    }

    #[test]
    fn test_extract_tool_calls_all_tool_types() {
        let steps = vec![
            json!({"type": "CORTEX_STEP_TYPE_VIEW_FILE", "data": {"path": "/test/file.rs"}}),
            json!({"type": "CORTEX_STEP_TYPE_LIST_DIRECTORY", "data": {"path": "/test"}}),
            json!({"type": "CORTEX_STEP_TYPE_EDIT_FILE", "data": {"path": "/test/file.rs", "content": "new content"}}),
            json!({"type": "CORTEX_STEP_TYPE_CREATE_FILE", "data": {"path": "/test/new.rs"}}),
            json!({"type": "CORTEX_STEP_TYPE_DELETE_FILE", "data": {"path": "/test/old.rs"}}),
            json!({"type": "CORTEX_STEP_TYPE_SHELL", "data": {"command": "ls -la"}}),
            json!({"type": "CORTEX_STEP_TYPE_GREP_SEARCH", "data": {"pattern": "test", "path": "/test"}}),
            json!({"type": "CORTEX_STEP_TYPE_RUN_COMMAND", "data": {"command": "cargo test"}}),
        ];
        let calls = extract_tool_calls_from_steps(&steps);
        assert_eq!(calls.len(), 8);
        assert_eq!(calls[0].function.name, "view_file");
        assert_eq!(calls[1].function.name, "list_directory");
        assert_eq!(calls[2].function.name, "edit_file");
        assert_eq!(calls[3].function.name, "create_file");
        assert_eq!(calls[4].function.name, "delete_file");
        assert_eq!(calls[5].function.name, "shell");
        assert_eq!(calls[6].function.name, "grep_search");
        assert_eq!(calls[7].function.name, "run_command");
    }

    #[test]
    fn test_extract_tool_calls_with_data_extraction() {
        let steps = vec![
            json!({
                "type": "CORTEX_STEP_TYPE_VIEW_FILE",
                "data": {
                    "path": "/home/user/project/src/main.rs",
                    "line": 42
                }
            }),
        ];
        let calls = extract_tool_calls_from_steps(&steps);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "view_file");
        // Verify data is serialized into arguments
        let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["path"], "/home/user/project/src/main.rs");
        assert_eq!(args["line"], 42);
    }

    #[test]
    fn test_extract_tool_calls_without_data_field() {
        // When no "data" field, should serialize the entire step
        let steps = vec![
            json!({
                "type": "CORTEX_STEP_TYPE_SHELL",
                "command": "echo hello",
                "exit_code": 0
            }),
        ];
        let calls = extract_tool_calls_from_steps(&steps);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "shell");
        let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["type"], "CORTEX_STEP_TYPE_SHELL");
        assert_eq!(args["command"], "echo hello");
    }

    #[test]
    fn test_extract_tool_calls_mixed_steps() {
        let steps = vec![
            json!({"type": "CORTEX_STEP_TYPE_PLANNER_RESPONSE"}),
            json!({"type": "CORTEX_STEP_TYPE_VIEW_FILE", "data": {"path": "/a"}}),
            json!({"type": "CORTEX_STEP_TYPE_PLANNER_RESPONSE"}),
            json!({"type": "CORTEX_STEP_TYPE_EDIT_FILE", "data": {"path": "/b"}}),
            json!({"type": "CORTEX_STEP_TYPE_FINISH"}),
        ];
        let calls = extract_tool_calls_from_steps(&steps);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "view_file");
        assert_eq!(calls[1].function.name, "edit_file");
    }

    #[test]
    fn test_extract_tool_calls_id_format() {
        let steps = vec![
            json!({"type": "CORTEX_STEP_TYPE_VIEW_FILE"}),
        ];
        let calls = extract_tool_calls_from_steps(&steps);
        assert_eq!(calls.len(), 1);
        // ID format: call_{index}_{uuid_short}
        assert!(calls[0].id.starts_with("call_0_"));
        assert!(calls[0].id.len() > 10); // Should have uuid part
    }

    #[test]
    fn test_extract_tool_calls_kind_is_function() {
        let steps = vec![
            json!({"type": "CORTEX_STEP_TYPE_SHELL"}),
        ];
        let calls = extract_tool_calls_from_steps(&steps);
        assert_eq!(calls[0].kind, "function");
    }
}
