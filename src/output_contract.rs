use anyhow::{anyhow, Context, Result};
use jsonschema::JSONSchema;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

use crate::settings::expand_path;

#[derive(Debug)]
pub struct ResultContract {
    schema_path: Option<PathBuf>,
    compiled_schema: Option<JSONSchema>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedAssistantOutput {
    pub value: Value,
    pub canonical_text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContractFailure {
    pub code: String,
    pub message: String,
    pub details: Value,
}

impl ResultContract {
    pub fn from_args(strict_json: bool, output_schema: Option<&str>) -> Result<Option<Self>> {
        let schema_path = output_schema
            .map(str::trim)
            .filter(|value| !value.is_empty());

        if let Some(raw_path) = schema_path {
            let expanded = if raw_path.starts_with("~/") {
                expand_path(raw_path)
            } else {
                PathBuf::from(raw_path)
            };
            let resolved = expanded.canonicalize().unwrap_or(expanded);
            if !resolved.exists() {
                return Err(anyhow!(
                    "output schema file not found: {}",
                    resolved.display()
                ));
            }
            let text = fs::read_to_string(&resolved)
                .with_context(|| format!("read {}", resolved.display()))?;
            let schema: Value = serde_json::from_str(&text)
                .with_context(|| format!("parse JSON schema from {}", resolved.display()))?;
            let compiled_schema = JSONSchema::compile(&schema)
                .map_err(|err| anyhow!("compile JSON schema from {}: {err}", resolved.display()))?;
            return Ok(Some(Self {
                schema_path: Some(resolved),
                compiled_schema: Some(compiled_schema),
            }));
        }

        if strict_json {
            return Ok(Some(Self {
                schema_path: None,
                compiled_schema: None,
            }));
        }

        Ok(None)
    }

    pub fn descriptor(&self) -> Value {
        match self.schema_path.as_ref() {
            Some(path) => json!({
                "type": "json_schema",
                "schemaPath": path.display().to_string(),
            }),
            None => json!({ "type": "strict_json" }),
        }
    }

    pub fn validate_output(
        &self,
        output_text: Option<&str>,
    ) -> std::result::Result<ValidatedAssistantOutput, ContractFailure> {
        let raw = output_text
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ContractFailure::new(
                    "missing_output",
                    "final assistant output is empty",
                    json!({}),
                )
            })?;

        let parse_target = extract_fenced_json(raw).unwrap_or(raw);
        let value: Value = serde_json::from_str(parse_target).map_err(|err| {
            ContractFailure::new(
                "invalid_json_output",
                format!("final assistant output is not valid JSON: {err}"),
                json!({
                    "line": err.line(),
                    "column": err.column(),
                }),
            )
        })?;

        if let Some(compiled_schema) = self.compiled_schema.as_ref() {
            if let Err(errors) = compiled_schema.validate(&value) {
                let validation_errors: Vec<String> =
                    errors.take(5).map(|err| err.to_string()).collect();
                return Err(ContractFailure::new(
                    "schema_validation_failed",
                    "final assistant output does not match the requested JSON schema",
                    json!({
                        "validationErrors": validation_errors,
                        "schemaPath": self
                            .schema_path
                            .as_ref()
                            .map(|path| path.display().to_string()),
                    }),
                ));
            }
        }

        Ok(ValidatedAssistantOutput {
            canonical_text: serde_json::to_string(&value).unwrap_or_else(|_| "null".to_string()),
            value,
        })
    }
}

fn extract_fenced_json(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    let without_closing = trimmed.strip_suffix("```")?;
    let without_opening = without_closing.strip_prefix("```")?;
    let newline_index = without_opening.find('\n')?;
    let header = without_opening[..newline_index].trim();
    if !header.is_empty() && !header.eq_ignore_ascii_case("json") {
        return None;
    }
    let body = without_opening[newline_index + 1..].trim();
    if body.is_empty() {
        return None;
    }
    Some(body)
}

impl ContractFailure {
    fn new(code: &str, message: impl Into<String>, details: Value) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            details,
        }
    }

    pub fn as_json(&self, descriptor: Value) -> Value {
        let mut payload = json!({
            "code": self.code.clone(),
            "message": self.message.clone(),
            "contract": descriptor,
        });
        if !self.details.is_null() && self.details != json!({}) {
            payload["details"] = self.details.clone();
        }
        payload
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn strict_json_accepts_valid_json() {
        let contract = ResultContract::from_args(true, None).unwrap().unwrap();
        let validated = contract
            .validate_output(Some("{\"ok\":true,\"score\":1}"))
            .unwrap();
        assert_eq!(validated.value, json!({ "ok": true, "score": 1 }));
        assert_eq!(validated.canonical_text, "{\"ok\":true,\"score\":1}");
    }

    #[test]
    fn strict_json_rejects_non_json_output() {
        let contract = ResultContract::from_args(true, None).unwrap().unwrap();
        let failure = contract.validate_output(Some("not json")).unwrap_err();
        assert_eq!(failure.code, "invalid_json_output");
    }

    #[test]
    fn strict_json_accepts_fenced_json_output() {
        let contract = ResultContract::from_args(true, None).unwrap().unwrap();
        let validated = contract
            .validate_output(Some("```json\n{\"ok\":true,\"score\":1}\n```"))
            .unwrap();
        assert_eq!(validated.value, json!({ "ok": true, "score": 1 }));
        assert_eq!(validated.canonical_text, "{\"ok\":true,\"score\":1}");
    }

    #[test]
    fn strict_json_rejects_text_outside_fenced_json_output() {
        let contract = ResultContract::from_args(true, None).unwrap().unwrap();
        let failure = contract
            .validate_output(Some("Here is the result:\n```json\n{\"ok\":true}\n```"))
            .unwrap_err();
        assert_eq!(failure.code, "invalid_json_output");
    }

    #[test]
    fn output_schema_rejects_shape_mismatch() {
        let temp_dir = TempDir::new().unwrap();
        let schema_path = temp_dir.path().join("result.schema.json");
        fs::write(
            &schema_path,
            serde_json::to_string(&json!({
                "type": "object",
                "required": ["answer"],
                "properties": {
                    "answer": { "type": "string" }
                },
                "additionalProperties": false
            }))
            .unwrap(),
        )
        .unwrap();

        let contract = ResultContract::from_args(false, Some(schema_path.to_str().unwrap()))
            .unwrap()
            .unwrap();
        let failure = contract
            .validate_output(Some("{\"answer\":1}"))
            .unwrap_err();

        assert_eq!(failure.code, "schema_validation_failed");
        assert!(!failure.details["validationErrors"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(
            failure.details["schemaPath"],
            json!(schema_path.canonicalize().unwrap().display().to_string())
        );
    }

    #[test]
    fn output_schema_accepts_fenced_json_output() {
        let temp_dir = TempDir::new().unwrap();
        let schema_path = temp_dir.path().join("result.schema.json");
        fs::write(
            &schema_path,
            serde_json::to_string(&json!({
                "type": "object",
                "required": ["answer"],
                "properties": {
                    "answer": { "type": "string" }
                },
                "additionalProperties": false
            }))
            .unwrap(),
        )
        .unwrap();

        let contract = ResultContract::from_args(false, Some(schema_path.to_str().unwrap()))
            .unwrap()
            .unwrap();
        let validated = contract
            .validate_output(Some("```json\n{\"answer\":\"ok\"}\n```"))
            .unwrap();

        assert_eq!(validated.value, json!({ "answer": "ok" }));
        assert_eq!(validated.canonical_text, "{\"answer\":\"ok\"}");
    }

    #[test]
    fn output_schema_requires_existing_file() {
        let temp_dir = TempDir::new().unwrap();
        let missing_path = temp_dir.path().join("missing.schema.json");
        let err =
            ResultContract::from_args(false, Some(missing_path.to_str().unwrap())).unwrap_err();
        assert!(err.to_string().contains("output schema file not found"));
    }
}
