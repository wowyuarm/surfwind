use crate::types::ModelInfo;

#[derive(Clone, Copy, Debug)]
struct PublicModelSpec {
    public_id: &'static str,
    default_runtime_id: &'static str,
    label: &'static str,
    provider: &'static str,
    aliases: &'static [&'static str],
    runtime_candidates: &'static [&'static str],
}

const PUBLIC_MODEL_SPECS: &[PublicModelSpec] = &[
    PublicModelSpec {
        public_id: "swe-1-6",
        default_runtime_id: "swe-1-6",
        label: "SWE-1.6",
        provider: "windsurf",
        aliases: &["swe-1-6", "swe-1.6", "swe16"],
        runtime_candidates: &["swe-1-6", "swe-1-6-fast"],
    },
    PublicModelSpec {
        public_id: "kimi-k2-5",
        default_runtime_id: "kimi-k2-5",
        label: "Kimi K2.5",
        provider: "moonshot",
        aliases: &["kimi-k2-5", "kimi-k2.5", "k2-5", "kimi"],
        runtime_candidates: &["kimi-k2-5"],
    },
    PublicModelSpec {
        public_id: "gemini-3-1-pro-high",
        default_runtime_id: "gemini-3-1-pro-high",
        label: "Gemini 3.1 Pro High Thinking",
        provider: "google",
        aliases: &[
            "gemini-3-1-pro-high",
            "gemini-3.1-pro-high",
            "gemini-3-1-pro",
            "gemini-3.1-pro",
        ],
        runtime_candidates: &["gemini-3-1-pro-high"],
    },
    PublicModelSpec {
        public_id: "claude-sonnet-4-6",
        default_runtime_id: "claude-sonnet-4-6",
        label: "Claude Sonnet 4.6",
        provider: "anthropic",
        aliases: &[
            "claude-sonnet-4-6",
            "claude-sonnet-4.6",
            "sonnet-4-6",
            "claude-4-6-sonnet",
        ],
        runtime_candidates: &["claude-sonnet-4-6"],
    },
    PublicModelSpec {
        public_id: "claude-opus-4-6",
        default_runtime_id: "claude-opus-4-6",
        label: "Claude Opus 4.6",
        provider: "anthropic",
        aliases: &[
            "claude-opus-4-6",
            "claude-opus-4.6",
            "opus-4-6",
            "claude-4-6-opus",
        ],
        runtime_candidates: &["claude-opus-4-6"],
    },
    PublicModelSpec {
        public_id: "gpt-5-4",
        default_runtime_id: "gpt-5-4-high",
        label: "GPT-5.4",
        provider: "openai",
        aliases: &[
            "gpt-5-4",
            "gpt-5.4",
            "gpt54",
            "gpt-5-4-high",
            "gpt-5.4-high",
        ],
        runtime_candidates: &["gpt-5-4-high", "gpt-5-4-high-priority"],
    },
    PublicModelSpec {
        public_id: "gpt-5-3-codex",
        default_runtime_id: "gpt-5-3-codex-high",
        label: "GPT-5.3 Codex",
        provider: "openai",
        aliases: &[
            "gpt-5-3-codex",
            "gpt-5.3-codex",
            "gpt53-codex",
            "gpt-5-3-codex-high",
            "gpt-5.3-codex-high",
        ],
        runtime_candidates: &["gpt-5-3-codex-high", "gpt-5-3-codex-high-priority"],
    },
];

pub fn public_model_catalog() -> Vec<ModelInfo> {
    PUBLIC_MODEL_SPECS.iter().map(spec_to_model_info).collect()
}

pub fn filter_public_models(discovered: &[ModelInfo]) -> Vec<ModelInfo> {
    let mut filtered = Vec::new();
    for spec in PUBLIC_MODEL_SPECS {
        if discovered
            .iter()
            .any(|model| matches_discovered_model(spec, &model.id))
        {
            filtered.push(spec_to_model_info(spec));
        }
    }
    filtered
}

pub fn resolve_requested_model_uid(model: Option<&str>, fallback: Option<&str>) -> String {
    model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| fallback.map(str::trim).filter(|value| !value.is_empty()))
        .map(resolve_requested_model_uid_value)
        .unwrap_or_else(|| "swe-1-6".to_string())
}

pub fn resolve_requested_model_uid_value(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "swe-1-6".to_string();
    }
    if let Some(spec) = find_requested_model_spec(trimmed) {
        spec.default_runtime_id.to_string()
    } else {
        trimmed.to_string()
    }
}

fn find_requested_model_spec(raw: &str) -> Option<&'static PublicModelSpec> {
    let normalized = normalize_model_key(raw);
    PUBLIC_MODEL_SPECS.iter().find(|spec| {
        normalize_model_key(spec.public_id) == normalized
            || spec
                .aliases
                .iter()
                .any(|alias| normalize_model_key(alias) == normalized)
    })
}

fn matches_discovered_model(spec: &PublicModelSpec, raw: &str) -> bool {
    let normalized = normalize_model_key(raw);
    normalize_model_key(spec.public_id) == normalized
        || spec
            .runtime_candidates
            .iter()
            .any(|candidate| normalize_model_key(candidate) == normalized)
}

fn normalize_model_key(raw: &str) -> String {
    let mut normalized = String::new();
    let mut last_was_dash = false;
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !normalized.is_empty() {
            normalized.push('-');
            last_was_dash = true;
        }
    }
    normalized.trim_matches('-').to_string()
}

fn spec_to_model_info(spec: &PublicModelSpec) -> ModelInfo {
    ModelInfo {
        id: spec.public_id.to_string(),
        object: "model".to_string(),
        owned_by: "surfwind".to_string(),
        label: Some(spec.label.to_string()),
        provider: Some(spec.provider.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_public_model_catalog_order() {
        let models = public_model_catalog();
        let ids = models.into_iter().map(|model| model.id).collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![
                "swe-1-6",
                "kimi-k2-5",
                "gemini-3-1-pro-high",
                "claude-sonnet-4-6",
                "claude-opus-4-6",
                "gpt-5-4",
                "gpt-5-3-codex",
            ]
        );
    }

    #[test]
    fn test_filter_public_models_curates_and_orders_results() {
        let discovered = vec![
            ModelInfo {
                id: "gpt-5-3-codex-high".to_string(),
                object: "model".to_string(),
                owned_by: "windsurf-local".to_string(),
                label: None,
                provider: None,
            },
            ModelInfo {
                id: "claude-sonnet-4-6".to_string(),
                object: "model".to_string(),
                owned_by: "windsurf-local".to_string(),
                label: None,
                provider: None,
            },
            ModelInfo {
                id: "gpt-5-4-high-priority".to_string(),
                object: "model".to_string(),
                owned_by: "windsurf-local".to_string(),
                label: None,
                provider: None,
            },
        ];
        let filtered = filter_public_models(&discovered);
        let ids = filtered
            .into_iter()
            .map(|model| model.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["claude-sonnet-4-6", "gpt-5-4", "gpt-5-3-codex"]);
    }

    #[test]
    fn test_resolve_requested_model_uid_maps_public_aliases() {
        assert_eq!(
            resolve_requested_model_uid_value("gpt-5.3-codex"),
            "gpt-5-3-codex-high"
        );
        assert_eq!(resolve_requested_model_uid_value("gpt-5-4"), "gpt-5-4-high");
        assert_eq!(
            resolve_requested_model_uid_value("claude-sonnet-4.6"),
            "claude-sonnet-4-6"
        );
    }

    #[test]
    fn test_resolve_requested_model_uid_preserves_unknown_values() {
        assert_eq!(
            resolve_requested_model_uid_value("custom-raw-model"),
            "custom-raw-model"
        );
    }
}
