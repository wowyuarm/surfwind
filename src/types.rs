use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputMode {
    Text,
    Json,
}

impl OutputMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
        }
    }

    pub fn parse(raw: Option<&str>) -> Self {
        match raw.unwrap_or("text").trim().to_ascii_lowercase().as_str() {
            "json" => Self::Json,
            _ => Self::Text,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub object: String,
    pub owned_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RuntimeState {
    pub api_key: String,
    pub ports: Vec<u16>,
    pub pid: i32,
    pub csrf: String,
    pub workspace_id: Option<String>,
    pub managed_by_surfwind: bool,
}

#[derive(Clone, Debug)]
pub struct RpcResponse {
    pub status: u16,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCallEnvelope {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolFunction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunRecord {
    #[serde(rename = "runId")]
    pub run_id: String,
    pub mode: String,
    pub path: String,
    #[serde(rename = "parentRunId", skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    pub prompt: String,
    #[serde(rename = "requestModel", skip_serializing_if = "Option::is_none")]
    pub request_model: Option<String>,
    #[serde(rename = "requestedModelUid")]
    pub requested_model_uid: String,
    #[serde(rename = "cascadeId", skip_serializing_if = "Option::is_none")]
    pub cascade_id: Option<String>,
    pub status: String,
    #[serde(rename = "httpStatus")]
    pub http_status: u16,
    #[serde(rename = "upstreamStatus", skip_serializing_if = "Option::is_none")]
    pub upstream_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(rename = "outputText", skip_serializing_if = "Option::is_none")]
    pub output_text: Option<String>,
    #[serde(rename = "toolCalls")]
    pub tool_calls: Vec<ToolCallEnvelope>,
    #[serde(rename = "stepOffset")]
    pub step_offset: usize,
    #[serde(rename = "newStepCount")]
    pub new_step_count: usize,
    #[serde(rename = "stepCount")]
    pub step_count: usize,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(rename = "completedAt", skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    pub summary: Value,
    pub events: Vec<Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunListItem {
    #[serde(rename = "runId")]
    pub run_id: String,
    pub mode: String,
    pub status: String,
    #[serde(rename = "parentRunId", skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    #[serde(rename = "requestedModelUid")]
    pub requested_model_uid: String,
    #[serde(rename = "cascadeId", skip_serializing_if = "Option::is_none")]
    pub cascade_id: Option<String>,
    #[serde(rename = "workspaceId", skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(rename = "outputPreview")]
    pub output_preview: String,
    #[serde(rename = "stepCount")]
    pub step_count: usize,
}

#[derive(Clone, Debug)]
pub struct AgentRunResult {
    pub status: u16,
    pub body: Value,
    pub run: RunRecord,
}
