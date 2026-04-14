// Test utilities shared across integration tests

use serde_json::json;
use surfwind::config::AppConfig;
use surfwind::settings::SettingsPaths;
use surfwind::types::RunRecord;
use tempfile::TempDir;

pub fn create_test_config(temp_dir: &TempDir) -> AppConfig {
    let home = temp_dir.path().join(".surfwind");
    AppConfig {
        paths: SettingsPaths {
            home_dir: home.clone(),
            settings_path: home.join("settings.json"),
            runs_dir: home.join("runs"),
            logs_dir: home.join("logs"),
            managed_runtimes_path: home.join("managed-runtimes.json"),
        },
        settings: surfwind::settings::SettingsData {
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

pub fn create_test_run(run_id: &str) -> RunRecord {
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
