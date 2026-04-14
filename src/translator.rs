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
                .and_then(|item| item.get("outputString"))
                .and_then(Value::as_str);
            if let Some(text) = out.filter(|value| !value.trim().is_empty()) {
                finish_outputs.push(text.trim().to_string());
            }
        }
    }

    for step in steps {
        if step.get("type").and_then(Value::as_str) == Some("CORTEX_STEP_TYPE_PLANNER_RESPONSE") {
            let planner = step.get("plannerResponse").and_then(Value::as_object);
            if let Some(planner) = planner {
                for key in ["response", "modifiedResponse"] {
                    if let Some(text) = planner
                        .get(key)
                        .and_then(Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                    {
                        planner_outputs.push(text.trim().to_string());
                    }
                }
                for key in ["outputPreview"] {
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
        let error_message = step.get("errorMessage").and_then(Value::as_object);
        let inner = error_message
            .and_then(|value| value.get("error"))
            .and_then(Value::as_object);
        for key in ["shortError", "userErrorMessage"] {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::settings::{SettingsData, SettingsPaths};
    use serde_json::json;
    use std::path::PathBuf;

    fn create_test_config() -> AppConfig {
        AppConfig {
            paths: SettingsPaths {
                home_dir: PathBuf::from("/tmp"),
                settings_path: PathBuf::from("/tmp/settings.json"),
                runs_dir: PathBuf::from("/tmp/runs"),
                logs_dir: PathBuf::from("/tmp/logs"),
                managed_runtimes_path: PathBuf::from("/tmp/managed-runtimes.json"),
            },
            settings: SettingsData {
                model: "test-model".to_string(),
                run_store_dir: "/tmp/runs".to_string(),
                output: "text".to_string(),
            },
            state_dir: PathBuf::from("/tmp/state"),
            user_settings_path: PathBuf::from("/tmp/user_settings.pb"),
            metadata_api_key: Some("test-api-key".to_string()),
            rpc_timeout_sec: 20.0,
            poll_interval_ms: 800,
            poll_max_rounds: 45,
            auto_launch_enabled: false,
            auto_launch_timeout_sec: 15.0,
            auto_launch_poll_interval_ms: 500,
            metadata_ide_name: "test-ide".to_string(),
            metadata_ide_version: "1.0.0".to_string(),
            metadata_extension_name: "test-ext".to_string(),
            metadata_extension_version: "2.0.0".to_string(),
            metadata_locale: "en-US".to_string(),
            metadata_os: "linux".to_string(),
        }
    }

    #[test]
    fn test_build_metadata() {
        let config = create_test_config();
        let metadata = build_metadata(&config, "api-key-456", "session-123");

        assert_eq!(metadata["ideName"], "test-ide");
        assert_eq!(metadata["ideVersion"], "1.0.0");
        assert_eq!(metadata["extensionName"], "test-ext");
        assert_eq!(metadata["extensionVersion"], "2.0.0");
        assert_eq!(metadata["apiKey"], "api-key-456");
        assert_eq!(metadata["sessionId"], "session-123");
        assert_eq!(metadata["locale"], "en-US");
        assert_eq!(metadata["os"], "linux");
    }

    #[test]
    fn test_extract_assistant_text_from_finish_step() {
        let steps = vec![json!({
            "type": "CORTEX_STEP_TYPE_FINISH",
            "finish": {
                "outputString": "  Hello from finish  "
            }
        })];

        assert_eq!(
            extract_assistant_text(&steps),
            Some("Hello from finish".to_string())
        );
    }

    #[test]
    fn test_extract_assistant_text_from_planner_response() {
        let steps = vec![json!({
            "type": "CORTEX_STEP_TYPE_PLANNER_RESPONSE",
            "plannerResponse": {
                "response": "  Planner response  "
            }
        })];

        assert_eq!(
            extract_assistant_text(&steps),
            Some("Planner response".to_string())
        );
    }

    #[test]
    fn test_extract_assistant_text_prefers_finish_over_planner() {
        let steps = vec![
            json!({
                "type": "CORTEX_STEP_TYPE_PLANNER_RESPONSE",
                "plannerResponse": {
                    "response": "Planner response"
                }
            }),
            json!({
                "type": "CORTEX_STEP_TYPE_FINISH",
                "finish": {
                    "outputString": "Finish response"
                }
            }),
        ];

        assert_eq!(
            extract_assistant_text(&steps),
            Some("Finish response".to_string())
        );
    }

    #[test]
    fn test_extract_assistant_text_from_preview() {
        let steps = vec![json!({
            "type": "CORTEX_STEP_TYPE_PLANNER_RESPONSE",
            "plannerResponse": {
                "outputPreview": "  Output preview  "
            }
        })];

        assert_eq!(
            extract_assistant_text(&steps),
            Some("Output preview".to_string())
        );
    }

    #[test]
    fn test_extract_assistant_text_empty_steps() {
        let steps: Vec<serde_json::Value> = vec![];
        assert_eq!(extract_assistant_text(&steps), None);
    }

    #[test]
    fn test_extract_error_short_from_error_step() {
        let steps = vec![json!({
            "type": "CORTEX_STEP_TYPE_ERROR_MESSAGE",
            "errorMessage": {
                "error": {
                    "shortError": "  Short error message  "
                }
            }
        })];

        assert_eq!(
            extract_error_short(&steps),
            Some("Short error message".to_string())
        );
    }

    #[test]
    fn test_extract_error_short_user_error_message() {
        let steps = vec![json!({
            "type": "CORTEX_STEP_TYPE_ERROR_MESSAGE",
            "errorMessage": {
                "error": {
                    "userErrorMessage": "User error message"
                }
            }
        })];

        assert_eq!(
            extract_error_short(&steps),
            Some("User error message".to_string())
        );
    }

    #[test]
    fn test_extract_error_short_fallback() {
        let steps = vec![json!({
            "type": "CORTEX_STEP_TYPE_ERROR_MESSAGE",
            "errorMessage": {
                "error": {}
            }
        })];

        assert_eq!(extract_error_short(&steps), Some("error_step".to_string()));
    }

    #[test]
    fn test_extract_error_short_no_error_step() {
        let steps = vec![json!({
            "type": "CORTEX_STEP_TYPE_FINISH",
            "finish": {}
        })];

        assert_eq!(extract_error_short(&steps), None);
    }

    #[test]
    fn test_latest_non_empty() {
        assert_eq!(latest_non_empty(&[]), None);
        assert_eq!(latest_non_empty(&["".to_string()]), None);
        assert_eq!(
            latest_non_empty(&["first".to_string(), "second".to_string()]),
            Some("second".to_string())
        );
        assert_eq!(
            latest_non_empty(&["first".to_string(), "".to_string(), "third".to_string()]),
            Some("third".to_string())
        );
    }
}
