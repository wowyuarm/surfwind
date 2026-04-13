use anyhow::{anyhow, Context, Result};
use dirs::home_dir;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::types::OutputMode;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SettingsData {
    pub model: String,
    #[serde(rename = "runStoreDir")]
    pub run_store_dir: String,
    pub output: String,
}

#[derive(Clone, Debug)]
pub struct SettingsPaths {
    pub home_dir: PathBuf,
    pub settings_path: PathBuf,
    pub runs_dir: PathBuf,
    pub logs_dir: PathBuf,
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

pub fn default_home_dir() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".surfwind")
}

pub fn resolve_paths() -> SettingsPaths {
    let home_dir = env_string(&["SURFWIND_HOME", "WINDSURF_HOME"])
        .map(PathBuf::from)
        .unwrap_or_else(default_home_dir);
    SettingsPaths {
        settings_path: home_dir.join("settings.json"),
        runs_dir: home_dir.join("runs"),
        logs_dir: home_dir.join("logs"),
        home_dir,
    }
}

pub fn default_settings(paths: &SettingsPaths) -> SettingsData {
    SettingsData {
        model: "kimi-k2-5".to_string(),
        run_store_dir: display_path(&paths.runs_dir),
        output: OutputMode::Text.as_str().to_string(),
    }
}

pub fn bootstrap(paths: &SettingsPaths) -> Result<()> {
    fs::create_dir_all(&paths.home_dir)
        .with_context(|| format!("create {}", paths.home_dir.display()))?;
    fs::create_dir_all(&paths.runs_dir)
        .with_context(|| format!("create {}", paths.runs_dir.display()))?;
    fs::create_dir_all(&paths.logs_dir)
        .with_context(|| format!("create {}", paths.logs_dir.display()))?;
    if !paths.settings_path.exists() {
        let data = serde_json::to_string_pretty(&default_settings(paths))?;
        fs::write(&paths.settings_path, data)
            .with_context(|| format!("write {}", paths.settings_path.display()))?;
    }
    Ok(())
}

pub fn load_settings(paths: &SettingsPaths) -> Result<SettingsData> {
    bootstrap(paths)?;
    let defaults = default_settings(paths);
    let text = fs::read_to_string(&paths.settings_path).unwrap_or_default();
    let raw: Value = serde_json::from_str(&text).unwrap_or_else(|_| Value::Object(Map::new()));
    Ok(normalize_settings(raw.as_object(), &defaults))
}

fn normalize_settings(raw: Option<&Map<String, Value>>, defaults: &SettingsData) -> SettingsData {
    let get_string = |key: &str, fallback: &str| -> String {
        raw.and_then(|obj| obj.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(fallback)
            .to_string()
    };

    let output = match get_string("output", &defaults.output)
        .to_ascii_lowercase()
        .as_str()
    {
        "json" => "json".to_string(),
        _ => defaults.output.clone(),
    };

    SettingsData {
        model: get_string("model", &defaults.model),
        run_store_dir: get_string("runStoreDir", &defaults.run_store_dir),
        output,
    }
}

pub fn read_setting(paths: &SettingsPaths, key: &str) -> Result<Option<Value>> {
    let text = fs::read_to_string(&paths.settings_path).unwrap_or_default();
    let raw: Value = serde_json::from_str(&text).unwrap_or_else(|_| Value::Object(Map::new()));
    let obj = raw.as_object().cloned().unwrap_or_default();
    validate_key(key)?;
    Ok(obj.get(key).cloned())
}

pub fn write_setting(paths: &SettingsPaths, key: &str, value: &str) -> Result<Value> {
    validate_key(key)?;
    bootstrap(paths)?;
    let text = fs::read_to_string(&paths.settings_path).unwrap_or_default();
    let raw: Value = serde_json::from_str(&text).unwrap_or_else(|_| Value::Object(Map::new()));
    let mut obj = raw.as_object().cloned().unwrap_or_default();
    let json_value = coerce_setting_value(key, value)?;
    obj.insert(key.to_string(), json_value.clone());
    fs::write(
        &paths.settings_path,
        serde_json::to_string_pretty(&Value::Object(obj))?,
    )?;
    Ok(json_value)
}

pub fn unset_setting(paths: &SettingsPaths, key: &str) -> Result<Value> {
    validate_key(key)?;
    bootstrap(paths)?;
    let text = fs::read_to_string(&paths.settings_path).unwrap_or_default();
    let raw: Value = serde_json::from_str(&text).unwrap_or_else(|_| Value::Object(Map::new()));
    let mut obj = raw.as_object().cloned().unwrap_or_default();
    obj.remove(key);
    fs::write(
        &paths.settings_path,
        serde_json::to_string_pretty(&Value::Object(obj))?,
    )?;
    let defaults = default_settings(paths);
    let value = match key {
        "model" => Value::String(defaults.model),
        "runStoreDir" => Value::String(defaults.run_store_dir),
        "output" => Value::String(defaults.output),
        _ => Value::Null,
    };
    Ok(value)
}

pub fn display_path(path: &Path) -> String {
    if let Some(home) = home_dir() {
        if let Ok(relative) = path.strip_prefix(&home) {
            if relative.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", relative.display());
        }
    }
    path.display().to_string()
}

pub fn expand_path(raw: &str) -> PathBuf {
    if let Some(stripped) = raw.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(stripped);
        }
    }
    if raw == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from("."));
    }
    PathBuf::from(raw)
}

fn validate_key(key: &str) -> Result<()> {
    match key {
        "model" | "runStoreDir" | "output" => Ok(()),
        _ => Err(anyhow!("unknown setting key: {}", key)),
    }
}

fn coerce_setting_value(key: &str, value: &str) -> Result<Value> {
    match key {
        "model" | "runStoreDir" => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Err(anyhow!("{} requires a non-empty value", key))
            } else {
                Ok(Value::String(trimmed.to_string()))
            }
        }
        "output" => match value.trim().to_ascii_lowercase().as_str() {
            "text" => Ok(Value::String("text".to_string())),
            "json" => Ok(Value::String("json".to_string())),
            _ => Err(anyhow!("output must be text or json")),
        },
        _ => Err(anyhow!("unknown setting key: {}", key)),
    }
}
