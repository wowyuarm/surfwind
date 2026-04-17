use anyhow::{anyhow, Context, Result};
use once_cell::sync::Lazy;
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;
use std::thread;
use std::time::Duration;

use crate::config::AppConfig;
use crate::models::filter_public_models;
use crate::types::{ModelInfo, RpcResponse, RuntimeState};

const LANGUAGE_SERVER_RPC_PATH: &str = "exa.language_server_pb.LanguageServerService";
// All language_server RPC endpoints are served on 127.0.0.1:<port>. Ignore any
// HTTP proxy environment variables so local RPC never gets tunneled through an
// external proxy (e.g. a developer-side http_proxy=http://127.0.0.1:7890), which
// would otherwise hijack runtime discovery with confusing 502s.
static HTTP_CLIENT: Lazy<Client> = Lazy::new(build_http_client);

fn build_http_client() -> Client {
    Client::builder()
        .no_proxy()
        .build()
        .expect("build reqwest blocking client without proxy")
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ActiveRuntimeContext {
    pub runtime: RuntimeState,
    pub active_port: u16,
    pub metadata: Value,
}

pub fn choose_active_port(
    config: &AppConfig,
    ports: &[u16],
    csrf: &str,
    metadata: &Value,
) -> Option<u16> {
    for round in 0..3 {
        for port in ports {
            if rpc_call(
                config,
                *port,
                csrf,
                "GetUserStatus",
                &json!({ "metadata": metadata }),
            )
            .status
                == 200
            {
                return Some(*port);
            }
        }
        if round < 2 {
            thread::sleep(Duration::from_millis(150));
        }
    }
    None
}

pub fn rpc_call(
    config: &AppConfig,
    port: u16,
    csrf: &str,
    method: &str,
    body: &Value,
) -> RpcResponse {
    let url = format!(
        "http://127.0.0.1:{}/{}/{}",
        port, LANGUAGE_SERVER_RPC_PATH, method
    );
    let response = HTTP_CLIENT
        .post(url)
        .header("content-type", "application/json")
        .header("x-codeium-csrf-token", csrf)
        .timeout(Duration::from_secs_f64(config.rpc_timeout_sec))
        .body(body.to_string())
        .send();

    match response {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let text = resp.text().unwrap_or_default();
            RpcResponse { status, text }
        }
        Err(err) => {
            if let Some(status) = err.status() {
                let text = err.to_string();
                RpcResponse {
                    status: status.as_u16(),
                    text,
                }
            } else {
                RpcResponse {
                    status: 0,
                    text: err.to_string(),
                }
            }
        }
    }
}

pub fn sample_outbound_targets(pid: i32) -> Vec<Value> {
    let text = match run_ss(&["-tnpH"]) {
        Ok(text) => text,
        Err(_) => return Vec::new(),
    };
    let mut counts: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for line in text.lines() {
        if !line.contains(&format!("pid={}", pid)) {
            continue;
        }
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }
        let state = parts[0].to_string();
        let peer = parts[4].to_string();
        let entry = counts.entry(peer).or_insert((state, 0));
        entry.1 += 1;
    }
    counts
        .into_iter()
        .map(|(peer, (state, count))| json!({ "peer": peer, "state": state, "count": count }))
        .collect()
}

pub fn discover_models(config: &AppConfig, active: &ActiveRuntimeContext) -> Vec<ModelInfo> {
    let response = rpc_call(
        config,
        active.active_port,
        &active.runtime.csrf,
        "GetUserSettings",
        &json!({ "metadata": active.metadata }),
    );
    if response.status != 200 {
        return config.default_models();
    }

    let payload: Value = serde_json::from_str(&response.text).unwrap_or(Value::Null);
    let discovered_rows = collect_model_objects(&payload);
    if !discovered_rows.is_empty() {
        let mut deduped: BTreeMap<String, ModelInfo> = BTreeMap::new();
        for row in discovered_rows {
            let key = row.id.clone();
            let replace = deduped
                .get(&key)
                .map(|current| score_model(current) <= score_model(&row))
                .unwrap_or(true);
            if replace {
                deduped.insert(key, decorate_model(row));
            }
        }
        let filtered = filter_public_models(&deduped.into_values().collect::<Vec<_>>());
        if !filtered.is_empty() {
            return filtered;
        }
        return config.default_models();
    }

    let mut ids = BTreeSet::new();
    collect_model_uids(&payload, &mut ids);
    if ids.is_empty() {
        return config.default_models();
    }
    let filtered = filter_public_models(
        &ids.into_iter()
            .map(|id| {
                decorate_model(ModelInfo {
                    id,
                    object: "model".to_string(),
                    owned_by: "windsurf-local".to_string(),
                    label: None,
                    provider: None,
                })
            })
            .collect::<Vec<_>>(),
    );
    if filtered.is_empty() {
        config.default_models()
    } else {
        filtered
    }
}

pub(crate) fn run_ss(args: &[&str]) -> Result<String> {
    let output = if which::which("rtk").is_ok() {
        Command::new("rtk")
            .arg("ss")
            .args(args)
            .output()
            .context("run rtk ss")?
    } else {
        Command::new("ss").args(args).output().context("run ss")?
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(stderr.trim().to_string()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub(crate) fn collect_model_objects(value: &Value) -> Vec<ModelInfo> {
    let mut rows = Vec::new();
    collect_model_objects_inner(value, &mut rows);
    rows
}

fn collect_model_objects_inner(value: &Value, rows: &mut Vec<ModelInfo>) {
    match value {
        Value::Object(map) => {
            if let Some(model_uid) = map
                .get("modelUid")
                .and_then(Value::as_str)
                .filter(|item| is_likely_selectable_model_uid(item))
            {
                rows.push(ModelInfo {
                    id: model_uid.to_string(),
                    object: "model".to_string(),
                    owned_by: "windsurf-local".to_string(),
                    label: map
                        .get("label")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    provider: map
                        .get("provider")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                });
            }
            for nested in map.values() {
                collect_model_objects_inner(nested, rows);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_model_objects_inner(item, rows);
            }
        }
        _ => {}
    }
}

fn collect_model_uids(value: &Value, found: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, nested) in map {
                if let Some(text) = nested.as_str() {
                    if is_likely_selectable_model_uid(text) {
                        found.insert(text.to_string());
                    }
                    if key.to_ascii_lowercase().contains("model")
                        && is_likely_selectable_model_uid(text)
                    {
                        found.insert(text.to_string());
                    }
                }
                collect_model_uids(nested, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_model_uids(item, found);
            }
        }
        _ => {}
    }
}

fn score_model(row: &ModelInfo) -> usize {
    usize::from(row.label.is_some()) + usize::from(row.provider.is_some())
}

fn decorate_model(mut row: ModelInfo) -> ModelInfo {
    if row
        .provider
        .as_deref()
        .map(|value| value.starts_with("MODEL_PROVIDER_"))
        .unwrap_or(false)
    {
        row.provider = row.provider.clone().map(|value| {
            value
                .trim_start_matches("MODEL_PROVIDER_")
                .to_ascii_lowercase()
        });
    } else if row
        .provider
        .as_deref()
        .map(|value| value.trim().is_empty())
        .unwrap_or(true)
    {
        row.provider = derive_provider_name(&row.id);
    }
    if row
        .label
        .as_deref()
        .map(|value| value.trim().is_empty())
        .unwrap_or(true)
    {
        row.label = Some(derive_model_label(&row.id));
    }
    row
}

fn derive_provider_name(model_id: &str) -> Option<String> {
    let lower = model_id.to_ascii_lowercase();
    if lower.contains("kimi") || lower.contains("moonshot") {
        Some("moonshot".to_string())
    } else if lower.contains("claude") {
        Some("anthropic".to_string())
    } else if lower.contains("gemini") {
        Some("google".to_string())
    } else if lower.contains("glm") {
        Some("glm".to_string())
    } else if lower.contains("grok") || lower.contains("xai") {
        Some("xai".to_string())
    } else if lower.contains("gpt") || lower.contains("o3") {
        Some("openai".to_string())
    } else if lower.contains("swe") || lower.contains("windsurf") {
        Some("windsurf".to_string())
    } else {
        None
    }
}

fn derive_model_label(model_id: &str) -> String {
    let mut text = model_id.trim().to_string();
    if let Some(stripped) = text.strip_prefix("MODEL_") {
        text = stripped.to_string();
    }
    for (source, target) in [
        ("CHAT_GPT_", "GPT "),
        ("CHAT_O3", "O3"),
        ("GOOGLE_GEMINI_", "Gemini "),
        ("XAI_GROK_", "Grok "),
        ("CLAUDE_", "Claude "),
        ("GLM_", "GLM "),
        ("SWE_", "SWE "),
    ] {
        if let Some(stripped) = text.strip_prefix(source) {
            text = format!("{}{}", target, stripped);
            break;
        }
    }
    let normalized_text = text.replace('_', " ").replace('-', " ");
    let parts: Vec<_> = normalized_text.split_whitespace().collect();
    let normalized: Vec<String> = parts
        .into_iter()
        .map(|part| {
            let upper = part.to_ascii_uppercase();
            if ["GPT", "O3", "GLM", "SWE", "BYOK", "XAI"].contains(&upper.as_str()) {
                upper
            } else {
                match part.to_ascii_lowercase().as_str() {
                    "kimi" => "Kimi".to_string(),
                    "gemini" => "Gemini".to_string(),
                    "claude" => "Claude".to_string(),
                    "grok" => "Grok".to_string(),
                    other => {
                        let mut chars = other.chars();
                        match chars.next() {
                            Some(first) => {
                                format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
                            }
                            None => String::new(),
                        }
                    }
                }
            }
        })
        .collect();
    let joined = normalized.join(" ");
    if joined.is_empty() {
        model_id.to_string()
    } else {
        joined
    }
}

pub(crate) fn is_likely_selectable_model_uid(model_uid: &str) -> bool {
    let candidate = model_uid.trim();
    if candidate.is_empty() {
        return false;
    }
    for blocked in [
        "COST_TIER",
        "DIMENSION_KIND",
        "PRIORITY",
        "MINIMAL",
        "PRICING_TYPE",
        "PROVIDER_",
        "MODEL_TYPE_",
    ] {
        if candidate.contains(blocked) {
            return false;
        }
    }
    if candidate.starts_with("MODEL_PRIVATE_") {
        return false;
    }
    if candidate.starts_with("MODEL_") {
        return true;
    }
    candidate.chars().any(|ch| ch.is_ascii_lowercase()) && candidate.contains('-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn test_collect_model_objects() {
        let value = json!({
            "models": [
                {"modelUid": "model-1", "object": "model"},
                {"modelUid": "model-2", "object": "model"}
            ]
        });
        let models = collect_model_objects(&value);
        assert_eq!(models.len(), 2);
    }

    #[test]
    fn test_is_likely_selectable_model_uid_true() {
        assert!(is_likely_selectable_model_uid("MODEL_abc-123"));
        assert!(is_likely_selectable_model_uid("swe-1-6"));
    }

    #[test]
    fn test_is_likely_selectable_model_uid_false() {
        assert!(!is_likely_selectable_model_uid("MODEL_PRIVATE_abc"));
        assert!(!is_likely_selectable_model_uid("gpt4"));
        assert!(!is_likely_selectable_model_uid(""));
        assert!(!is_likely_selectable_model_uid("PRIORITY_HIGH"));
    }

    #[test]
    fn build_http_client_ignores_proxy_env_for_localhost() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        listener
            .set_nonblocking(false)
            .expect("set blocking listener");

        let (tx, rx) = mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = b"{\"ok\":true}";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(body);
                let _ = stream.flush();
                let _ = tx.send(());
            }
        });

        let original_http_proxy = std::env::var("HTTP_PROXY").ok();
        let original_https_proxy = std::env::var("HTTPS_PROXY").ok();
        std::env::set_var("HTTP_PROXY", "http://127.0.0.1:1");
        std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:1");

        let client = build_http_client();
        let url = format!("http://127.0.0.1:{}/ping", port);
        let response = client
            .post(url)
            .timeout(Duration::from_secs(2))
            .body("{}")
            .send();

        if let Some(value) = original_http_proxy {
            std::env::set_var("HTTP_PROXY", value);
        } else {
            std::env::remove_var("HTTP_PROXY");
        }
        if let Some(value) = original_https_proxy {
            std::env::set_var("HTTPS_PROXY", value);
        } else {
            std::env::remove_var("HTTPS_PROXY");
        }

        let response = response.expect("loopback request must bypass proxy env vars");
        assert_eq!(response.status().as_u16(), 200);
        assert!(rx.recv_timeout(Duration::from_secs(2)).is_ok());
        handle.join().expect("listener thread");
    }
}
