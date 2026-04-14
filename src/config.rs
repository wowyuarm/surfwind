use std::path::PathBuf;

use anyhow::Result;

use crate::models::{public_model_catalog, resolve_requested_model_uid_value};
use crate::settings::{expand_path, load_settings, resolve_paths, SettingsData, SettingsPaths};
use crate::types::{ModelInfo, OutputMode};

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub paths: SettingsPaths,
    pub settings: SettingsData,
    pub state_dir: PathBuf,
    pub user_settings_path: PathBuf,
    pub metadata_api_key: Option<String>,
    pub rpc_timeout_sec: f64,
    pub poll_interval_ms: u64,
    pub poll_max_rounds: usize,
    pub auto_launch_enabled: bool,
    pub auto_launch_timeout_sec: f64,
    pub auto_launch_poll_interval_ms: u64,
    pub metadata_ide_name: String,
    pub metadata_ide_version: String,
    pub metadata_extension_name: String,
    pub metadata_extension_version: String,
    pub metadata_locale: String,
    pub metadata_os: String,
}

fn env_string(keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn env_bool(keys: &[&str], default: bool) -> bool {
    for key in keys {
        if let Ok(value) = std::env::var(key) {
            match value.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => return true,
                "0" | "false" | "no" | "off" => return false,
                _ => {}
            }
        }
    }
    default
}

fn env_f64(keys: &[&str], default: f64) -> f64 {
    env_string(keys)
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(default)
}

fn env_u64(keys: &[&str], default: u64) -> u64 {
    env_string(keys)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        let paths = resolve_paths();
        let settings = load_settings(&paths)?;
        let state_dir = env_string(&["SURFWIND_STATE_DIR", "WINDSURF_STATE_DIR"])
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("~/.windsurf-server/data"))
            .to_string_lossy()
            .to_string();
        let user_settings_path =
            env_string(&["SURFWIND_USER_SETTINGS_PATH", "WINDSURF_USER_SETTINGS_PATH"])
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("~/.codeium/windsurf/user_settings.pb"))
                .to_string_lossy()
                .to_string();

        Ok(Self {
            paths,
            settings: settings.clone(),
            state_dir: expand_path(&state_dir),
            user_settings_path: expand_path(&user_settings_path),
            metadata_api_key: env_string(&[
                "SURFWIND_METADATA_API_KEY",
                "WINDSURF_METADATA_API_KEY",
            ]),
            rpc_timeout_sec: env_f64(
                &["SURFWIND_RPC_TIMEOUT_SEC", "WINDSURF_RPC_TIMEOUT_SEC"],
                20.0,
            ),
            poll_interval_ms: env_u64(
                &["SURFWIND_POLL_INTERVAL_MS", "WINDSURF_POLL_INTERVAL_MS"],
                800,
            ),
            poll_max_rounds: env_u64(
                &["SURFWIND_POLL_MAX_ROUNDS", "WINDSURF_POLL_MAX_ROUNDS"],
                45,
            ) as usize,
            auto_launch_enabled: env_bool(&["SURFWIND_AUTO_LAUNCH", "WINDSURF_AUTO_LAUNCH"], true),
            auto_launch_timeout_sec: env_f64(
                &[
                    "SURFWIND_AUTO_LAUNCH_TIMEOUT_SEC",
                    "WINDSURF_AUTO_LAUNCH_TIMEOUT_SEC",
                ],
                15.0,
            ),
            auto_launch_poll_interval_ms: env_u64(
                &[
                    "SURFWIND_AUTO_LAUNCH_POLL_INTERVAL_MS",
                    "WINDSURF_AUTO_LAUNCH_POLL_INTERVAL_MS",
                ],
                500,
            ),
            metadata_ide_name: env_string(&["SURFWIND_IDE_NAME", "WINDSURF_IDE_NAME"])
                .unwrap_or_else(|| "windsurf".to_string()),
            metadata_ide_version: env_string(&["SURFWIND_IDE_VERSION", "WINDSURF_IDE_VERSION"])
                .unwrap_or_else(|| "1.110.1".to_string()),
            metadata_extension_name: env_string(&[
                "SURFWIND_EXTENSION_NAME",
                "WINDSURF_EXTENSION_NAME",
            ])
            .unwrap_or_else(|| "windsurf".to_string()),
            metadata_extension_version: env_string(&[
                "SURFWIND_EXTENSION_VERSION",
                "WINDSURF_EXTENSION_VERSION",
            ])
            .unwrap_or_else(|| "1.0.0".to_string()),
            metadata_locale: env_string(&["SURFWIND_LOCALE", "WINDSURF_LOCALE"])
                .unwrap_or_else(|| "en".to_string()),
            metadata_os: env_string(&["SURFWIND_OS", "WINDSURF_OS"])
                .unwrap_or_else(|| std::env::consts::OS.to_string()),
        })
    }

    pub fn default_model_uid(&self) -> String {
        resolve_requested_model_uid_value(
            env_string(&["SURFWIND_MODEL_UID", "WINDSURF_MODEL_UID"])
                .unwrap_or_else(|| self.settings.model.clone())
                .as_str(),
        )
    }

    pub fn default_output(&self) -> OutputMode {
        OutputMode::parse(
            env_string(&["SURFWIND_OUTPUT", "WINDSURF_OUTPUT"])
                .as_deref()
                .or(Some(self.settings.output.as_str())),
        )
    }

    pub fn run_store_dir(&self) -> PathBuf {
        let raw = env_string(&["SURFWIND_RUN_STORE_DIR", "WINDSURF_RUN_STORE_DIR"])
            .unwrap_or_else(|| self.settings.run_store_dir.clone());
        expand_path(&raw)
    }

    pub fn default_models(&self) -> Vec<ModelInfo> {
        public_model_catalog()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
                output: "json".to_string(),
            },
            state_dir: PathBuf::from("/tmp/state"),
            user_settings_path: PathBuf::from("/tmp/user_settings.pb"),
            metadata_api_key: Some("test-key".to_string()),
            rpc_timeout_sec: 20.0,
            poll_interval_ms: 800,
            poll_max_rounds: 45,
            auto_launch_enabled: false,
            auto_launch_timeout_sec: 15.0,
            auto_launch_poll_interval_ms: 500,
            metadata_ide_name: "test-ide".to_string(),
            metadata_ide_version: "1.0.0".to_string(),
            metadata_extension_name: "test-ext".to_string(),
            metadata_extension_version: "1.0.0".to_string(),
            metadata_locale: "en".to_string(),
            metadata_os: "linux".to_string(),
        }
    }

    #[test]
    fn test_default_model_uid() {
        let config = create_test_config();
        assert_eq!(config.default_model_uid(), "test-model");
    }

    #[test]
    fn test_default_model_uid_resolves_public_alias() {
        let mut config = create_test_config();
        config.settings.model = "gpt-5-4".to_string();
        assert_eq!(config.default_model_uid(), "gpt-5-4-high");
    }

    #[test]
    fn test_default_output() {
        let config = create_test_config();
        assert_eq!(config.default_output(), OutputMode::Json);
    }

    #[test]
    fn test_run_store_dir() {
        let config = create_test_config();
        let path = config.run_store_dir();
        assert!(path.to_string_lossy().contains("runs"));
    }

    #[test]
    fn test_default_models() {
        let config = create_test_config();
        let models = config.default_models();
        assert_eq!(models.len(), 7);
        assert_eq!(models[0].id, "swe-1-6");
        assert_eq!(models[0].owned_by, "surfwind");
        assert_eq!(models[5].id, "gpt-5-4");
        assert_eq!(models[6].id, "gpt-5-3-codex");
    }

    #[test]
    fn test_env_model_override() {
        std::env::set_var("SURFWIND_MODEL_UID", "env-model");
        let config = create_test_config();
        assert_eq!(config.default_model_uid(), "env-model");
        std::env::remove_var("SURFWIND_MODEL_UID");
    }
}
