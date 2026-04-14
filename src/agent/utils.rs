// Agent utilities - pure helper functions (no side effects)

use serde_json::{json, Value};
use uuid::Uuid;

use crate::models::resolve_requested_model_uid as resolve_shared_model_uid;

/// Generate a new unique run ID
pub fn new_run_id() -> String {
    format!("surf-run-{}", &Uuid::new_v4().simple().to_string()[..12])
}

/// Get requested model UID with fallback
pub fn requested_model_uid(model: Option<&str>, fallback: Option<&str>) -> String {
    resolve_shared_model_uid(model, fallback)
}

/// Build status label from HTTP status and content
pub fn status_label(
    http_status: u16,
    output_text: Option<&str>,
    error_text: Option<&str>,
) -> String {
    if http_status >= 400 {
        return "failed".to_string();
    }
    if error_text.is_some() {
        return "error".to_string();
    }
    match http_status {
        200 => {
            if output_text.map_or(false, |t| !t.is_empty()) {
                "completed".to_string()
            } else {
                "empty".to_string()
            }
        }
        202 => "running".to_string(),
        _ => "unknown".to_string(),
    }
}

/// Truncate text to a limit
pub fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// Check if status is terminal
pub fn is_terminal_status(status: Option<&str>) -> bool {
    matches!(status, Some("completed") | Some("failed") | Some("error"))
}

/// Check if status is running
pub fn is_running_status(status: Option<&str>) -> bool {
    matches!(status, Some("running") | Some("pending"))
}

/// Safe JSON object parsing
pub fn safe_json_object(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| json!({}))
}

/// Slice steps from offset
pub fn slice_steps(steps: Option<&Vec<Value>>, step_offset: usize) -> Vec<Value> {
    steps
        .map(|vec| vec.iter().skip(step_offset).cloned().collect())
        .unwrap_or_default()
}

/// Prefer assistant text (non-empty over empty)
pub fn prefer_assistant_text(current: Option<String>, candidate: Option<String>) -> Option<String> {
    match (current, candidate) {
        (Some(curr), Some(cand)) if curr.trim().is_empty() && !cand.trim().is_empty() => Some(cand),
        (Some(curr), _) if !curr.trim().is_empty() => Some(curr),
        (_, Some(cand)) if !cand.trim().is_empty() => Some(cand),
        _ => None,
    }
}

/// Create an event with timestamp
pub fn event(event_type: &str, data: Value, now_iso: &str) -> Value {
    json!({
        "type": event_type,
        "ts": now_iso,
        "data": data,
    })
}

/// Inject workspace fence into prompt
pub fn inject_workspace_fence(prompt: &str, workspace_root: Option<&str>) -> String {
    match workspace_root {
        Some(root)
            if !root.is_empty()
                && !prompt.contains(&format!("<workspace_root>{root}</workspace_root>")) =>
        {
            format!("<workspace_root>{root}</workspace_root>\n{prompt}")
        }
        _ => prompt.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_run_id_format() {
        let id = new_run_id();
        assert!(id.starts_with("surf-run-"));
        assert_eq!(id.len(), 21); // "surf-run-" (9) + 12 chars from uuid
    }

    #[test]
    fn test_requested_model_uid_no_fallback() {
        assert_eq!(requested_model_uid(None, None), "swe-1-6");
    }

    #[test]
    fn test_requested_model_uid_with_model() {
        assert_eq!(requested_model_uid(Some("gpt-4"), None), "gpt-4");
    }

    #[test]
    fn test_requested_model_uid_with_fallback() {
        assert_eq!(
            requested_model_uid(None, Some("fallback-model")),
            "fallback-model"
        );
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
    fn test_status_label_200_empty() {
        assert_eq!(status_label(200, Some(""), None), "empty");
    }

    #[test]
    fn test_status_label_200_none() {
        assert_eq!(status_label(200, None, None), "empty");
    }

    #[test]
    fn test_status_label_400() {
        assert_eq!(status_label(400, Some("text"), None), "failed");
    }

    #[test]
    fn test_status_label_with_error() {
        assert_eq!(status_label(200, Some("text"), Some("error")), "error");
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 3), "hel");
        assert_eq!(truncate("hi", 10), "hi");
    }

    #[test]
    fn test_is_terminal_status() {
        assert!(is_terminal_status(Some("completed")));
        assert!(is_terminal_status(Some("failed")));
        assert!(is_terminal_status(Some("error")));
        assert!(!is_terminal_status(Some("running")));
        assert!(!is_terminal_status(None));
    }

    #[test]
    fn test_is_running_status() {
        assert!(is_running_status(Some("running")));
        assert!(is_running_status(Some("pending")));
        assert!(!is_running_status(Some("completed")));
        assert!(!is_running_status(None));
    }

    #[test]
    fn test_safe_json_object_valid() {
        let result = safe_json_object(r#"{"key": "value"}"#);
        assert_eq!(result["key"], "value");
    }

    #[test]
    fn test_safe_json_object_invalid() {
        let result = safe_json_object("not json");
        assert_eq!(result, json!({}));
    }

    #[test]
    fn test_slice_steps() {
        let steps = vec![json!(1), json!(2), json!(3), json!(4)];
        let sliced = slice_steps(Some(&steps), 2);
        assert_eq!(sliced.len(), 2);
        assert_eq!(sliced[0], json!(3));
    }

    #[test]
    fn test_slice_steps_none() {
        let sliced: Vec<Value> = slice_steps(None, 0);
        assert!(sliced.is_empty());
    }

    #[test]
    fn test_prefer_assistant_text_non_empty_wins() {
        assert_eq!(
            prefer_assistant_text(Some("".to_string()), Some("content".to_string())),
            Some("content".to_string())
        );
    }

    #[test]
    fn test_prefer_assistant_text_current_kept() {
        assert_eq!(
            prefer_assistant_text(Some("existing".to_string()), Some("new".to_string())),
            Some("existing".to_string())
        );
    }

    #[test]
    fn test_event_structure() {
        let data = json!({"test": "data"});
        let evt = event("test.event", data.clone(), "2024-01-01T00:00:00Z");
        assert_eq!(evt["type"], "test.event");
        assert_eq!(evt["ts"], "2024-01-01T00:00:00Z");
        assert_eq!(evt["data"], data);
    }

    #[test]
    fn test_inject_workspace_fence() {
        let result = inject_workspace_fence("prompt", Some("/workspace"));
        assert_eq!(
            result,
            "<workspace_root>/workspace</workspace_root>\nprompt"
        );
    }

    #[test]
    fn test_inject_workspace_fence_no_fence() {
        let prompt = "<workspace_root>/workspace</workspace_root>\nprompt";
        let result = inject_workspace_fence(prompt, Some("/workspace"));
        assert_eq!(result, prompt); // unchanged
    }

    #[test]
    fn test_inject_workspace_fence_no_workspace() {
        let result = inject_workspace_fence("prompt", None);
        assert_eq!(result, "prompt");
    }
}
