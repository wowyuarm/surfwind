use std::path::PathBuf;

use anyhow::Result;

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
        env_string(&["SURFWIND_MODEL_UID", "WINDSURF_MODEL_UID"])
            .unwrap_or_else(|| self.settings.model.clone())
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
        vec![ModelInfo {
            id: self.default_model_uid(),
            object: "model".to_string(),
            owned_by: "windsurf-local".to_string(),
            label: None,
            provider: None,
        }]
    }
}
