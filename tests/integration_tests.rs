mod common;

use surfwind::settings::{bootstrap, expand_path, load_settings, SettingsPaths};
use surfwind::types::OutputMode;
use surfwind::runstore::{save_run, get_run, list_runs, summarize_run};
use surfwind::translator::{build_metadata, extract_assistant_text, extract_error_short};
use serde_json::json;
use tempfile::TempDir;

use common::{create_test_config, create_test_run};

#[test]
fn test_settings_bootstrap_creates_directories() {
    let temp_dir = TempDir::new().unwrap();
    let paths = SettingsPaths {
        home_dir: temp_dir.path().join(".surfwind"),
        settings_path: temp_dir.path().join(".surfwind/settings.json"),
        runs_dir: temp_dir.path().join(".surfwind/runs"),
        logs_dir: temp_dir.path().join(".surfwind/logs"),
        managed_runtimes_path: temp_dir.path().join(".surfwind/managed-runtimes.json"),
    };

    bootstrap(&paths).unwrap();

    assert!(paths.home_dir.exists());
    assert!(paths.runs_dir.exists());
    assert!(paths.logs_dir.exists());
    assert!(paths.settings_path.exists());
}

#[test]
fn test_settings_load_and_persist() {
    let temp_dir = TempDir::new().unwrap();
    let paths = SettingsPaths {
        home_dir: temp_dir.path().join(".surfwind"),
        settings_path: temp_dir.path().join(".surfwind/settings.json"),
        runs_dir: temp_dir.path().join(".surfwind/runs"),
        logs_dir: temp_dir.path().join(".surfwind/logs"),
        managed_runtimes_path: temp_dir.path().join(".surfwind/managed-runtimes.json"),
    };

    bootstrap(&paths).unwrap();
    let settings = load_settings(&paths).unwrap();

    assert_eq!(settings.model, "swe-1-6");
    assert_eq!(settings.output, "text");
}

#[test]
fn test_expand_path_with_tilde() {
    let expanded = expand_path("~/test/path");
    assert!(!expanded.to_string_lossy().contains("~"));
}

#[test]
fn test_run_store_end_to_end() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_test_config(&temp_dir);

    // Save multiple runs
    let run1 = create_test_run("run-1");
    let run2 = create_test_run("run-2");
    let run3 = create_test_run("run-3");

    save_run(&config, &run1).unwrap();
    save_run(&config, &run2).unwrap();
    save_run(&config, &run3).unwrap();

    // Retrieve a specific run
    let retrieved = get_run(&config, "run-2").unwrap();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().run_id, "run-2");

    // List all runs
    let runs = list_runs(&config, 10).unwrap();
    assert_eq!(runs.len(), 3);

    // Test summarize_run
    let summary = summarize_run(&run1);
    assert_eq!(summary.run_id, "run-1");
    assert_eq!(summary.status, "completed");
}

#[test]
fn test_translator_metadata_building() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_test_config(&temp_dir);

    let metadata = build_metadata(&config, "api-key", "session-123");

    assert_eq!(metadata["ideName"], "test");
    assert_eq!(metadata["sessionId"], "session-123");
    assert_eq!(metadata["apiKey"], "api-key");
}

#[test]
fn test_translator_extract_assistant_text_integration() {
    // Test with multiple step types
    let steps = vec![
        json!({
            "type": "CORTEX_STEP_TYPE_PLANNER_RESPONSE",
            "plannerResponse": {
                "response": "Planner says hello"
            }
        }),
        json!({
            "type": "CORTEX_STEP_TYPE_FINISH",
            "finish": {
                "outputString": "Final answer"
            }
        }),
    ];

    let text = extract_assistant_text(&steps);
    assert_eq!(text, Some("Final answer".to_string()));
}

#[test]
fn test_translator_extract_error_integration() {
    let steps = vec![
        json!({
            "type": "CORTEX_STEP_TYPE_PLANNER_RESPONSE",
            "plannerResponse": {
                "response": "Some response"
            }
        }),
        json!({
            "type": "CORTEX_STEP_TYPE_ERROR_MESSAGE",
            "errorMessage": {
                "error": {
                    "shortError": "Something went wrong"
                }
            }
        }),
    ];

    let error = extract_error_short(&steps);
    assert_eq!(error, Some("Something went wrong".to_string()));
}

#[test]
fn test_output_mode_parsing() {
    assert_eq!(OutputMode::parse(Some("text")), OutputMode::Text);
    assert_eq!(OutputMode::parse(Some("json")), OutputMode::Json);
    assert_eq!(OutputMode::parse(Some("stream-json")), OutputMode::StreamJson);
    assert_eq!(OutputMode::parse(Some("jsonl")), OutputMode::StreamJson);
    assert_eq!(OutputMode::parse(Some("unknown")), OutputMode::Text);
    assert_eq!(OutputMode::parse(None), OutputMode::Text);
}
