use serde_json::{json, Value};

use crate::config::AppConfig;

pub fn build_metadata(config: &AppConfig, api_key: &str, session_id: &str) -> Value {
    json!({
        "ideName": config.metadata_ide_name,
        "ideVersion": config.metadata_ide_version,
        "extensionName": config.metadata_extension_name,
        "extensionVersion": config.metadata_extension_version,
        "apiKey": api_key,
        "sessionId": session_id,
        "locale": config.metadata_locale,
        "os": config.metadata_os,
    })
}

pub fn extract_assistant_text(steps: &[Value]) -> Option<String> {
    let mut finish_outputs = Vec::new();
    let mut planner_outputs = Vec::new();
    let mut preview_outputs = Vec::new();

    for step in steps {
        if step.get("type").and_then(Value::as_str) == Some("CORTEX_STEP_TYPE_FINISH") {
            let finish = step.get("finish").and_then(Value::as_object);
            let out = finish
                .and_then(|item| {
                    item.get("outputString")
                        .or_else(|| item.get("output_string"))
                })
                .and_then(Value::as_str);
            if let Some(text) = out.filter(|value| !value.trim().is_empty()) {
                finish_outputs.push(text.trim().to_string());
            }
        }
    }

    for step in steps {
        if step.get("type").and_then(Value::as_str) == Some("CORTEX_STEP_TYPE_PLANNER_RESPONSE") {
            let planner = step
                .get("plannerResponse")
                .or_else(|| step.get("planner_response"))
                .and_then(Value::as_object);
            if let Some(planner) = planner {
                for key in ["response", "modifiedResponse", "modified_response"] {
                    if let Some(text) = planner
                        .get(key)
                        .and_then(Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                    {
                        planner_outputs.push(text.trim().to_string());
                    }
                }
                for key in ["outputPreview", "preview"] {
                    if let Some(text) = planner
                        .get(key)
                        .and_then(Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                    {
                        preview_outputs.push(text.trim().to_string());
                    }
                }
            }
        }
    }

    latest_non_empty(&finish_outputs)
        .or_else(|| latest_non_empty(&planner_outputs))
        .or_else(|| latest_non_empty(&preview_outputs))
}

pub fn extract_error_short(steps: &[Value]) -> Option<String> {
    for step in steps {
        if step.get("type").and_then(Value::as_str) != Some("CORTEX_STEP_TYPE_ERROR_MESSAGE") {
            continue;
        }
        let error_message = step
            .get("errorMessage")
            .or_else(|| step.get("error_message"))
            .and_then(Value::as_object);
        let inner = error_message
            .and_then(|value| value.get("error"))
            .and_then(Value::as_object);
        for key in [
            "shortError",
            "short_error",
            "userErrorMessage",
            "user_error_message",
        ] {
            if let Some(text) = inner
                .and_then(|value| value.get(key))
                .and_then(Value::as_str)
            {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        return Some("error_step".to_string());
    }
    None
}

fn latest_non_empty(values: &[String]) -> Option<String> {
    values
        .iter()
        .rev()
        .find(|item| !item.trim().is_empty())
        .cloned()
}
