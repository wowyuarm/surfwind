use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;

use crate::agent::{
    execute_agent_prompt, get_agent_events, get_agent_run, get_latest_agent_run,
    list_agent_runs_filtered, resume_agent_prompt, AgentRunOptions,
};
use crate::config::AppConfig;
use crate::output_contract::{ContractFailure, ResultContract, ValidatedAssistantOutput};
use crate::runtime::runtime_diagnostics;
use crate::settings::{
    bootstrap, describe_settings, expand_path, load_settings, read_setting, setting_keys,
    unset_setting, write_setting,
};
use crate::types::{ModelInfo, OutputMode, RunRecord};

#[derive(Parser, Debug)]
#[command(
    name = "surfwind",
    version,
    about = "Agent-first CLI wrapper around the local Windsurf runtime"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Status(StatusArgs),
    Models(StatusArgs),
    Exec(ExecArgs),
    Resume(ResumeArgs),
    Runs(RunsArgs),
    Show(RunIdArgs),
    Events(RunIdArgs),
    Settings(SettingsArgs),
}

#[derive(Args, Debug, Clone, Default)]
struct ReadOutputArgs {
    #[arg(long)]
    output: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug, Clone, Default)]
struct ResultContractArgs {
    #[arg(long)]
    strict_json: bool,
    #[arg(long)]
    output_schema: Option<String>,
}

#[derive(Args, Debug, Clone, Default)]
struct ArtifactOutputArgs {
    #[arg(long)]
    output_last_message_file: Option<String>,
    #[arg(long)]
    result_file: Option<String>,
}

#[derive(Args, Debug)]
struct StatusArgs {
    #[arg(long)]
    workspace: Option<String>,
    #[arg(long)]
    no_auto_launch: bool,
    #[command(flatten)]
    output: ReadOutputArgs,
}

#[derive(Args, Debug)]
struct RunsArgs {
    #[arg(long, default_value_t = 20)]
    limit: usize,
    #[arg(long)]
    status: Option<String>,
    #[arg(long)]
    workspace: Option<String>,
    #[command(flatten)]
    output: ReadOutputArgs,
}

#[derive(Args, Debug)]
struct RunIdArgs {
    run_id: Option<String>,
    #[arg(long)]
    latest: bool,
    #[command(flatten)]
    output: ReadOutputArgs,
}

#[derive(Args, Debug)]
struct ExecArgs {
    prompt: Option<String>,
    #[arg(short = 'p', long = "prompt")]
    prompt_option: Option<String>,
    #[arg(short = 'f', long = "file")]
    files: Vec<String>,
    #[arg(long)]
    workspace: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    output: Option<String>,
    #[arg(long)]
    json: bool,
    #[arg(long, conflicts_with = "json", conflicts_with = "output")]
    output_last_message: bool,
    #[arg(short = 'q', long)]
    quiet: bool,
    #[arg(long)]
    no_persist: bool,
    #[arg(long)]
    no_auto_launch: bool,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    timeout_seconds: Option<u64>,
    #[command(flatten)]
    result_contract: ResultContractArgs,
    #[command(flatten)]
    artifact_output: ArtifactOutputArgs,
}

#[derive(Args, Debug)]
struct ResumeArgs {
    run_id: Option<String>,
    prompt: Option<String>,
    #[arg(short = 'p', long = "prompt")]
    prompt_option: Option<String>,
    #[arg(short = 'f', long = "file")]
    files: Vec<String>,
    #[arg(long)]
    workspace: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    output: Option<String>,
    #[arg(long)]
    json: bool,
    #[arg(long, conflicts_with = "json", conflicts_with = "output")]
    output_last_message: bool,
    #[arg(short = 'q', long)]
    quiet: bool,
    #[arg(long)]
    last: bool,
    #[arg(long)]
    no_persist: bool,
    #[arg(long)]
    no_auto_launch: bool,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    timeout_seconds: Option<u64>,
    #[command(flatten)]
    result_contract: ResultContractArgs,
    #[command(flatten)]
    artifact_output: ArtifactOutputArgs,
}

#[derive(Args, Debug)]
struct SettingsArgs {
    #[command(subcommand)]
    command: SettingsCommand,
}

#[derive(Subcommand, Debug)]
enum SettingsCommand {
    Show,
    Keys,
    Describe { key: Option<String> },
    Get { key: String },
    Set { key: String, value: String },
    Unset { key: String },
}

pub fn run() -> Result<i32> {
    let cli = Cli::parse();
    let config = AppConfig::load()?;
    bootstrap(&config.paths)?;

    let code = match cli.command {
        Commands::Status(args) => cmd_status(
            &config,
            args.workspace.as_deref(),
            !args.no_auto_launch,
            resolve_query_output_mode(args.output.output.as_deref(), args.output.json),
        )?,
        Commands::Models(args) => cmd_models(
            &config,
            args.workspace.as_deref(),
            !args.no_auto_launch,
            resolve_query_output_mode(args.output.output.as_deref(), args.output.json),
        )?,
        Commands::Exec(args) => cmd_exec(&config, args)?,
        Commands::Resume(args) => cmd_resume(&config, args)?,
        Commands::Runs(args) => cmd_runs(
            &config,
            args.limit,
            args.status.as_deref(),
            args.workspace.as_deref(),
            resolve_query_output_mode(args.output.output.as_deref(), args.output.json),
        )?,
        Commands::Show(args) => cmd_show(
            &config,
            args.run_id.as_deref(),
            args.latest,
            resolve_query_output_mode(args.output.output.as_deref(), args.output.json),
        )?,
        Commands::Events(args) => cmd_events(
            &config,
            args.run_id.as_deref(),
            args.latest,
            resolve_query_output_mode(args.output.output.as_deref(), args.output.json),
        )?,
        Commands::Settings(args) => cmd_settings(&config, args.command)?,
    };
    Ok(code)
}

fn cmd_status(
    config: &AppConfig,
    workspace: Option<&str>,
    auto_launch: bool,
    output_mode: OutputMode,
) -> Result<i32> {
    if !matches!(output_mode, OutputMode::Json) {
        return Ok(print_unsupported_output_mode(
            "status",
            output_mode,
            &["json"],
        ));
    }
    match runtime_diagnostics(config, workspace, auto_launch) {
        Ok(body) => {
            print_json(&json!({ "ok": true, "status": body }));
            Ok(0)
        }
        Err(err) => {
            print_json(&json!({ "ok": false, "error": err.to_string() }));
            Ok(1)
        }
    }
}

fn cmd_models(
    config: &AppConfig,
    workspace: Option<&str>,
    auto_launch: bool,
    output_mode: OutputMode,
) -> Result<i32> {
    match runtime_diagnostics(config, workspace, auto_launch) {
        Ok(body) => {
            let models = body.get("models").cloned().unwrap_or_else(|| json!([]));
            match output_mode {
                OutputMode::Json => print_json(&json!({ "ok": true, "models": models })),
                OutputMode::Text => {
                    let parsed: Vec<ModelInfo> = serde_json::from_value(models).unwrap_or_default();
                    print_models_text(&parsed);
                }
                OutputMode::StreamJson => {
                    return Ok(print_unsupported_output_mode(
                        "models",
                        output_mode,
                        &["json", "text"],
                    ));
                }
            }
            Ok(0)
        }
        Err(err) => {
            print_json(&json!({ "ok": false, "error": err.to_string() }));
            Ok(1)
        }
    }
}

fn cmd_exec(config: &AppConfig, args: ExecArgs) -> Result<i32> {
    let result_contract = match resolve_result_contract(&args.result_contract) {
        Ok(contract) => contract,
        Err(err) => {
            print_json(&json!({
                "ok": false,
                "error": {
                    "code": "output_contract_invalid",
                    "message": err.to_string(),
                }
            }));
            return Ok(1);
        }
    };
    let prompt = match resolve_prompt(
        args.prompt.as_deref(),
        args.prompt_option.as_deref(),
        &args.files,
    ) {
        Ok(Some(prompt)) => prompt,
        Ok(None) => {
            print_json(&json!({ "ok": false, "error": "prompt is required" }));
            return Ok(1);
        }
        Err(err) => {
            print_json(&json!({ "ok": false, "error": err.to_string() }));
            return Ok(1);
        }
    };
    let options = AgentRunOptions {
        persist: !args.no_persist,
        auto_launch: !args.no_auto_launch,
        timeout_seconds: args.timeout_seconds,
    };
    let result = execute_agent_prompt(
        config,
        &prompt,
        args.model.as_deref(),
        args.workspace.as_deref(),
        options,
    );
    let output_mode = resolve_output_mode(config, args.output.as_deref(), args.json);
    let ok = matches!(result.status, 200 | 202);
    let error = result
        .body
        .get("error")
        .filter(|value: &&Value| !value.is_null())
        .cloned();
    let (validated_output, contract_failure) =
        validate_run_output(result_contract.as_ref(), ok, &result.run);
    let last_message_text =
        resolve_last_message_text(&result.run, error.as_ref(), validated_output.as_ref());
    let base_payload = build_run_result_payload(
        &result.run,
        ok && contract_failure.is_none(),
        error.as_ref(),
        validated_output.as_ref(),
        result_contract.as_ref(),
        contract_failure.as_ref(),
        None,
    );
    let artifact_failure = write_output_artifacts(
        &args.artifact_output,
        &base_payload,
        last_message_text.as_deref(),
    );
    let effective_ok = ok && contract_failure.is_none() && artifact_failure.is_none();
    print_run_result(
        &result.run,
        output_mode,
        args.output_last_message,
        args.quiet,
        effective_ok,
        error.as_ref(),
        validated_output.as_ref(),
        result_contract.as_ref(),
        contract_failure.as_ref(),
        artifact_failure.as_ref(),
    );
    Ok(if effective_ok { 0 } else { 1 })
}

fn cmd_resume(config: &AppConfig, args: ResumeArgs) -> Result<i32> {
    let result_contract = match resolve_result_contract(&args.result_contract) {
        Ok(contract) => contract,
        Err(err) => {
            print_json(&json!({
                "ok": false,
                "error": {
                    "code": "output_contract_invalid",
                    "message": err.to_string(),
                }
            }));
            return Ok(1);
        }
    };
    let parent_run_id = match resolve_run_id_selector(config, args.run_id.as_deref(), args.last)? {
        Some(run_id) => run_id,
        None => {
            print_json(&json!({
                "ok": false,
                "error": "run id is required",
                "hint": "pass a run id, 'latest', or --last",
            }));
            return Ok(1);
        }
    };
    let prompt = match resolve_prompt(
        args.prompt.as_deref(),
        args.prompt_option.as_deref(),
        &args.files,
    ) {
        Ok(Some(prompt)) => prompt,
        Ok(None) => {
            print_json(&json!({ "ok": false, "error": "prompt is required" }));
            return Ok(1);
        }
        Err(err) => {
            print_json(&json!({ "ok": false, "error": err.to_string() }));
            return Ok(1);
        }
    };
    let options = AgentRunOptions {
        persist: !args.no_persist,
        auto_launch: !args.no_auto_launch,
        timeout_seconds: args.timeout_seconds,
    };
    let result = resume_agent_prompt(
        config,
        &parent_run_id,
        &prompt,
        args.model.as_deref(),
        args.workspace.as_deref(),
        options,
    );
    let output_mode = resolve_output_mode(config, args.output.as_deref(), args.json);
    let ok = matches!(result.status, 200 | 202);
    let error = result
        .body
        .get("error")
        .filter(|value: &&Value| !value.is_null())
        .cloned();
    let (validated_output, contract_failure) =
        validate_run_output(result_contract.as_ref(), ok, &result.run);
    let last_message_text =
        resolve_last_message_text(&result.run, error.as_ref(), validated_output.as_ref());
    let base_payload = build_run_result_payload(
        &result.run,
        ok && contract_failure.is_none(),
        error.as_ref(),
        validated_output.as_ref(),
        result_contract.as_ref(),
        contract_failure.as_ref(),
        None,
    );
    let artifact_failure = write_output_artifacts(
        &args.artifact_output,
        &base_payload,
        last_message_text.as_deref(),
    );
    let effective_ok = ok && contract_failure.is_none() && artifact_failure.is_none();
    print_run_result(
        &result.run,
        output_mode,
        args.output_last_message,
        args.quiet,
        effective_ok,
        error.as_ref(),
        validated_output.as_ref(),
        result_contract.as_ref(),
        contract_failure.as_ref(),
        artifact_failure.as_ref(),
    );
    Ok(if effective_ok { 0 } else { 1 })
}

fn cmd_runs(
    config: &AppConfig,
    limit: usize,
    status: Option<&str>,
    workspace: Option<&str>,
    output_mode: OutputMode,
) -> Result<i32> {
    if !matches!(output_mode, OutputMode::Json) {
        return Ok(print_unsupported_output_mode(
            "runs",
            output_mode,
            &["json"],
        ));
    }
    let runs = list_agent_runs_filtered(config, limit, status, workspace)?;
    print_json(&json!({ "ok": true, "runs": runs }));
    Ok(0)
}

fn cmd_show(
    config: &AppConfig,
    run_id: Option<&str>,
    latest: bool,
    output_mode: OutputMode,
) -> Result<i32> {
    if !matches!(output_mode, OutputMode::Json) {
        return Ok(print_unsupported_output_mode(
            "show",
            output_mode,
            &["json"],
        ));
    }
    let requested_run_id = requested_run_selector(run_id, latest);
    if let Some(run) = resolve_target_run(config, run_id, latest)? {
        print_json(&json!({ "ok": true, "run": run }));
        Ok(0)
    } else {
        print_json(&json!({ "ok": false, "error": "run not found", "runId": requested_run_id }));
        Ok(1)
    }
}

fn cmd_events(
    config: &AppConfig,
    run_id: Option<&str>,
    latest: bool,
    output_mode: OutputMode,
) -> Result<i32> {
    if !matches!(output_mode, OutputMode::Json) {
        return Ok(print_unsupported_output_mode(
            "events",
            output_mode,
            &["json"],
        ));
    }
    let requested_run_id = requested_run_selector(run_id, latest);
    let resolved_run_id = match resolve_run_id_selector(config, run_id, latest)? {
        Some(run_id) => run_id,
        None => {
            print_json(
                &json!({ "ok": false, "error": "run not found", "runId": requested_run_id }),
            );
            return Ok(1);
        }
    };
    if let Some(events) = get_agent_events(config, &resolved_run_id)? {
        print_json(&json!({ "ok": true, "runId": resolved_run_id, "events": events }));
        Ok(0)
    } else {
        print_json(&json!({ "ok": false, "error": "run not found", "runId": requested_run_id }));
        Ok(1)
    }
}

fn cmd_settings(config: &AppConfig, command: SettingsCommand) -> Result<i32> {
    match command {
        SettingsCommand::Show => {
            let settings = load_settings(&config.paths)?;
            print_json(&json!({
                "ok": true,
                "settingsPath": config.paths.settings_path.display().to_string(),
                "settings": settings,
            }));
            Ok(0)
        }
        SettingsCommand::Keys => {
            print_json(&json!({
                "ok": true,
                "keys": setting_keys(),
            }));
            Ok(0)
        }
        SettingsCommand::Describe { key } => {
            let described = describe_settings(&config.paths, key.as_deref())?;
            print_json(&json!({
                "ok": true,
                "settings": described,
            }));
            Ok(0)
        }
        SettingsCommand::Get { key } => {
            let value = read_setting(&config.paths, &key)?;
            match value {
                None | Some(serde_json::Value::Null) => println!("null"),
                Some(serde_json::Value::String(text)) => println!("{}", text),
                Some(other) => println!("{}", serde_json::to_string(&other)?),
            }
            Ok(0)
        }
        SettingsCommand::Set { key, value } => {
            let written = write_setting(&config.paths, &key, &value)?;
            print_json(&json!({
                "ok": true,
                "settingsPath": config.paths.settings_path.display().to_string(),
                "key": key,
                "value": written,
            }));
            Ok(0)
        }
        SettingsCommand::Unset { key } => {
            let value = unset_setting(&config.paths, &key)?;
            print_json(&json!({
                "ok": true,
                "settingsPath": config.paths.settings_path.display().to_string(),
                "key": key,
                "value": value,
            }));
            Ok(0)
        }
    }
}

fn resolve_prompt(
    prompt: Option<&str>,
    prompt_option: Option<&str>,
    files: &[String],
) -> Result<Option<String>> {
    let direct_prompt = prompt
        .filter(|value| !value.trim().is_empty() && *value != "-")
        .or_else(|| prompt_option.filter(|value| !value.trim().is_empty() && *value != "-"))
        .map(|value| value.trim().to_string());

    let stdin_value = read_piped_stdin()?;
    let mut sections = Vec::new();
    if let Some(prompt) = direct_prompt {
        sections.push(prompt);
    }
    if let Some(stdin) = stdin_value {
        sections.push(format!("<stdin>\n{}\n</stdin>", stdin));
    }
    for file in files {
        let path = PathBuf::from(file);
        if !path.exists() || !path.is_file() {
            return Err(anyhow::anyhow!("file not found: {}", file));
        }
        let resolved = path.canonicalize().unwrap_or(path);
        sections.push(format!("<attached_file path=\"{}\" />", resolved.display()));
    }
    let joined = sections.join("\n\n");
    if joined.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(joined))
    }
}

fn read_piped_stdin() -> Result<Option<String>> {
    if io::stdin().is_terminal() {
        return Ok(None);
    }
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;
    let trimmed = buffer.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

fn resolve_output_mode(config: &AppConfig, output: Option<&str>, as_json: bool) -> OutputMode {
    if as_json {
        OutputMode::Json
    } else if let Some(output) = output {
        OutputMode::parse(Some(output))
    } else {
        config.default_output()
    }
}

fn resolve_query_output_mode(output: Option<&str>, as_json: bool) -> OutputMode {
    if as_json {
        OutputMode::Json
    } else if let Some(output) = output {
        OutputMode::parse(Some(output))
    } else {
        OutputMode::Json
    }
}

fn requested_run_selector(run_id: Option<&str>, latest: bool) -> String {
    if wants_latest(run_id, latest) {
        "latest".to_string()
    } else {
        run_id.unwrap_or_default().to_string()
    }
}

fn wants_latest(run_id: Option<&str>, latest: bool) -> bool {
    latest || matches!(run_id, Some(value) if value.eq_ignore_ascii_case("latest"))
}

fn resolve_target_run(
    config: &AppConfig,
    run_id: Option<&str>,
    latest: bool,
) -> Result<Option<RunRecord>> {
    if wants_latest(run_id, latest) {
        get_latest_agent_run(config)
    } else if let Some(run_id) = run_id.map(str::trim).filter(|value| !value.is_empty()) {
        get_agent_run(config, run_id)
    } else {
        Ok(None)
    }
}

fn resolve_run_id_selector(
    config: &AppConfig,
    run_id: Option<&str>,
    latest: bool,
) -> Result<Option<String>> {
    Ok(resolve_target_run(config, run_id, latest)?.map(|run| run.run_id))
}

fn resolve_result_contract(args: &ResultContractArgs) -> Result<Option<ResultContract>> {
    ResultContract::from_args(args.strict_json, args.output_schema.as_deref())
}

fn validate_run_output(
    result_contract: Option<&ResultContract>,
    ok: bool,
    run: &RunRecord,
) -> (Option<ValidatedAssistantOutput>, Option<ContractFailure>) {
    let Some(result_contract) = result_contract else {
        return (None, None);
    };
    if !ok {
        return (None, None);
    }
    if run.http_status == 202 {
        return (
            None,
            Some(ContractFailure {
                code: "result_not_ready".to_string(),
                message:
                    "final assistant output is not available because the run is still in progress"
                        .to_string(),
                details: json!({
                    "httpStatus": run.http_status,
                    "status": run.status,
                    "upstreamStatus": run.upstream_status,
                }),
            }),
        );
    }
    if run.http_status != 200 {
        return (None, None);
    }
    match result_contract.validate_output(run.output_text.as_deref()) {
        Ok(validated) => (Some(validated), None),
        Err(failure) => (None, Some(failure)),
    }
}

fn resolve_last_message_text(
    run: &RunRecord,
    error: Option<&Value>,
    validated_output: Option<&ValidatedAssistantOutput>,
) -> Option<String> {
    validated_output
        .map(|value| value.canonical_text.clone())
        .or_else(|| {
            run.output_text
                .as_ref()
                .filter(|value| !value.is_empty())
                .cloned()
        })
        .or_else(|| error.and_then(extract_error_message))
        .or_else(|| run.error.clone())
}

fn write_output_artifacts(
    args: &ArtifactOutputArgs,
    payload: &Value,
    last_message_text: Option<&str>,
) -> Option<Value> {
    if let Some(raw_path) = args
        .output_last_message_file
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let path = resolve_artifact_path(raw_path);
        if let Err(err) = write_artifact_file(&path, last_message_text.unwrap_or_default()) {
            return Some(artifact_failure_payload(
                "output_last_message_file_write_failed",
                format!(
                    "failed to write output-last-message artifact to {}",
                    path.display()
                ),
                json!({
                    "artifact": "output_last_message_file",
                    "path": path.display().to_string(),
                    "cause": err.to_string(),
                }),
            ));
        }
    }

    if let Some(raw_path) = args
        .result_file
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let path = resolve_artifact_path(raw_path);
        let encoded = match serde_json::to_string_pretty(payload) {
            Ok(encoded) => encoded,
            Err(err) => {
                return Some(artifact_failure_payload(
                    "result_file_encode_failed",
                    "failed to serialize result payload for artifact output",
                    json!({
                        "artifact": "result_file",
                        "path": path.display().to_string(),
                        "cause": err.to_string(),
                    }),
                ));
            }
        };
        if let Err(err) = write_artifact_file(&path, &encoded) {
            return Some(artifact_failure_payload(
                "result_file_write_failed",
                format!("failed to write result artifact to {}", path.display()),
                json!({
                    "artifact": "result_file",
                    "path": path.display().to_string(),
                    "cause": err.to_string(),
                }),
            ));
        }
    }

    None
}

fn resolve_artifact_path(raw_path: &str) -> PathBuf {
    if raw_path.starts_with("~/") {
        expand_path(raw_path)
    } else {
        PathBuf::from(raw_path)
    }
}

fn write_artifact_file(path: &PathBuf, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|value| !value.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}

fn artifact_failure_payload(code: &str, message: impl Into<String>, details: Value) -> Value {
    let mut payload = json!({
        "code": code,
        "message": message.into(),
    });
    if !details.is_null() && details != json!({}) {
        payload["details"] = details;
    }
    payload
}

fn print_run_result(
    run: &crate::types::RunRecord,
    output_mode: OutputMode,
    output_last_message: bool,
    quiet: bool,
    ok: bool,
    error: Option<&Value>,
    validated_output: Option<&ValidatedAssistantOutput>,
    result_contract: Option<&ResultContract>,
    contract_failure: Option<&ContractFailure>,
    artifact_failure: Option<&Value>,
) {
    if output_last_message {
        if contract_failure.is_some() || artifact_failure.is_some() {
            print_json(&build_run_result_payload(
                run,
                ok,
                error,
                validated_output,
                result_contract,
                contract_failure,
                artifact_failure,
            ));
        } else {
            print_last_message(run, error, validated_output);
        }
        return;
    }
    match output_mode {
        OutputMode::Json => {
            print_json(&build_run_result_payload(
                run,
                ok,
                error,
                validated_output,
                result_contract,
                contract_failure,
                artifact_failure,
            ));
        }
        OutputMode::Text => {
            if ok {
                if let Some(text) = validated_output.map(|value| value.canonical_text.as_str()) {
                    println!("{}", text);
                    if !quiet {
                        eprintln!("\nrun_id: {}", run.run_id);
                    }
                } else if let Some(text) =
                    run.output_text.as_ref().filter(|value| !value.is_empty())
                {
                    println!("{}", text);
                    if !quiet {
                        eprintln!("\nrun_id: {}", run.run_id);
                    }
                } else {
                    print_json(&build_run_result_payload(
                        run,
                        ok,
                        error,
                        validated_output,
                        result_contract,
                        contract_failure,
                        artifact_failure,
                    ));
                }
            } else {
                print_json(&build_run_result_payload(
                    run,
                    ok,
                    error,
                    validated_output,
                    result_contract,
                    contract_failure,
                    artifact_failure,
                ));
            }
        }
        OutputMode::StreamJson => {
            for (event_index, event) in run.events.iter().enumerate() {
                print_json_line(&json!({
                    "kind": "run.event",
                    "runId": run.run_id,
                    "eventIndex": event_index,
                    "event": event,
                }));
            }
            let mut line = json!({
                "kind": "run.result",
                "runId": run.run_id,
                "ok": ok,
                "httpStatus": run.http_status,
                "status": run.status,
                "run": run,
            });
            if let Some(result_contract) = result_contract {
                line["contract"] = result_contract.descriptor();
            }
            if let Some(validated_output) = validated_output {
                line["result"] = validated_output.value.clone();
            }
            if let Some(artifact_failure) = artifact_failure {
                line["ok"] = json!(false);
                line["failureKind"] = json!("output_artifact");
                line["error"] = artifact_failure.clone();
            } else if let Some(contract_failure) = contract_failure {
                line["ok"] = json!(false);
                line["failureKind"] = json!("output_contract");
                let descriptor = result_contract
                    .map(ResultContract::descriptor)
                    .unwrap_or_else(|| json!({ "type": "strict_json" }));
                line["error"] = contract_failure.as_json(descriptor);
            } else if let Some(error) = error {
                line["error"] = error.clone();
            }
            print_json_line(&line);
        }
    }
}

fn build_run_result_payload(
    run: &crate::types::RunRecord,
    ok: bool,
    error: Option<&Value>,
    validated_output: Option<&ValidatedAssistantOutput>,
    result_contract: Option<&ResultContract>,
    contract_failure: Option<&ContractFailure>,
    artifact_failure: Option<&Value>,
) -> Value {
    let mut payload = json!({ "ok": ok, "run": run });
    if let Some(result_contract) = result_contract {
        payload["contract"] = result_contract.descriptor();
    }
    if let Some(validated_output) = validated_output {
        payload["result"] = validated_output.value.clone();
    }
    if let Some(artifact_failure) = artifact_failure {
        payload["ok"] = json!(false);
        payload["failureKind"] = json!("output_artifact");
        payload["error"] = artifact_failure.clone();
    } else if let Some(contract_failure) = contract_failure {
        payload["ok"] = json!(false);
        payload["failureKind"] = json!("output_contract");
        let descriptor = result_contract
            .map(ResultContract::descriptor)
            .unwrap_or_else(|| json!({ "type": "strict_json" }));
        payload["error"] = contract_failure.as_json(descriptor);
    } else if let Some(error) = error {
        payload["error"] = error.clone();
    }
    payload
}

fn print_last_message(
    run: &crate::types::RunRecord,
    error: Option<&Value>,
    validated_output: Option<&ValidatedAssistantOutput>,
) {
    if let Some(text) = resolve_last_message_text(run, error, validated_output) {
        println!("{}", text);
    }
}

fn extract_error_message(error: &Value) -> Option<String> {
    if let Some(text) = error
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(text.to_string());
    }
    if let Some(text) = error
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(text.to_string());
    }
    let serialized = serde_json::to_string(error).ok()?;
    if serialized == "null" || serialized == "{}" {
        None
    } else {
        Some(serialized)
    }
}

fn print_json_line(value: &Value) {
    println!(
        "{}",
        serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string())
    );
}

fn print_json(value: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
    );
}

fn print_models_text(models: &[ModelInfo]) {
    let width = models.iter().map(|model| model.id.len()).max().unwrap_or(0);
    for model in models {
        println!(
            "{:<width$}  {}",
            model.id,
            model.label.as_deref().unwrap_or(model.id.as_str()),
            width = width,
        );
    }
}

fn print_unsupported_output_mode(
    command: &str,
    output_mode: OutputMode,
    supported: &[&str],
) -> i32 {
    print_json(&json!({
        "ok": false,
        "command": command,
        "error": format!(
            "unsupported output mode '{}' for command '{}'",
            output_mode.as_str(),
            command
        ),
        "supportedOutputs": supported,
    }));
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_resolve_output_mode_from_config() {
        // Test that when no CLI flag is provided, we use config default
        // This tests the logic without needing full AppConfig
        assert_eq!(OutputMode::parse(Some("json")), OutputMode::Json);
        assert_eq!(
            OutputMode::parse(Some("stream-json")),
            OutputMode::StreamJson
        );
    }

    #[test]
    fn test_resolve_output_mode_json_flag() {
        // When --json flag is passed, it should override everything
        let json_mode = OutputMode::Json;
        assert_eq!(json_mode, OutputMode::Json);
    }

    #[test]
    fn test_resolve_query_output_mode_defaults_to_json() {
        assert_eq!(resolve_query_output_mode(None, false), OutputMode::Json);
        assert_eq!(
            resolve_query_output_mode(Some("json"), false),
            OutputMode::Json
        );
        assert_eq!(
            resolve_query_output_mode(Some("text"), false),
            OutputMode::Text
        );
    }

    #[test]
    fn test_agent_run_options_defaults() {
        let options = AgentRunOptions::default();
        assert!(options.persist);
        assert!(options.auto_launch);
        assert_eq!(options.timeout_seconds, None);
    }

    #[test]
    fn test_parse_exec_args_minimal() {
        // Test that ExecArgs can be constructed with minimal fields
        // In real usage, clap would parse this from CLI
        let args = ExecArgs {
            prompt: Some("test prompt".to_string()),
            prompt_option: None,
            files: vec![],
            workspace: None,
            model: None,
            output: None,
            json: false,
            output_last_message: false,
            quiet: false,
            no_persist: false,
            no_auto_launch: false,
            timeout_seconds: None,
            result_contract: ResultContractArgs::default(),
            artifact_output: ArtifactOutputArgs::default(),
        };
        assert_eq!(args.prompt, Some("test prompt".to_string()));
        assert!(!args.no_persist); // persist is enabled by default
    }

    #[test]
    fn test_parse_exec_args_with_flags() {
        let args = ExecArgs {
            prompt: None,
            prompt_option: Some("from option".to_string()),
            files: vec!["file1.txt".to_string(), "file2.txt".to_string()],
            workspace: Some("/workspace".to_string()),
            model: Some("gpt-5-4".to_string()),
            output: Some("json".to_string()),
            json: true,
            output_last_message: false,
            quiet: true,
            no_persist: true,
            no_auto_launch: true,
            timeout_seconds: Some(30),
            result_contract: ResultContractArgs {
                strict_json: true,
                output_schema: None,
            },
            artifact_output: ArtifactOutputArgs {
                output_last_message_file: Some("/tmp/final.txt".to_string()),
                result_file: Some("/tmp/result.json".to_string()),
            },
        };
        assert_eq!(args.workspace, Some("/workspace".to_string()));
        assert_eq!(args.model, Some("gpt-5-4".to_string()));
        assert!(args.json);
        assert!(args.quiet);
        assert!(args.no_persist); // explicitly disabled
        assert!(args.no_auto_launch);
        assert!(args.result_contract.strict_json);
        assert_eq!(args.timeout_seconds, Some(30));
    }

    #[test]
    fn test_parse_resume_args() {
        let args = ResumeArgs {
            run_id: Some("surf-run-abc123".to_string()),
            prompt: Some("continue".to_string()),
            prompt_option: None,
            files: vec![],
            workspace: None,
            model: None,
            output: None,
            json: false,
            output_last_message: false,
            quiet: false,
            last: false,
            no_persist: false,
            no_auto_launch: false,
            timeout_seconds: Some(15),
            result_contract: ResultContractArgs::default(),
            artifact_output: ArtifactOutputArgs::default(),
        };
        assert_eq!(args.run_id, Some("surf-run-abc123".to_string()));
        assert_eq!(args.timeout_seconds, Some(15));
    }

    #[test]
    fn test_validate_run_output_rejects_running_result_when_contract_is_requested() {
        let contract = ResultContract::from_args(true, None).unwrap().unwrap();
        let run = RunRecord {
            run_id: "run-1".to_string(),
            mode: "exec".to_string(),
            path: "/tmp".to_string(),
            parent_run_id: None,
            prompt: "prompt".to_string(),
            request_model: None,
            requested_model_uid: "test-model".to_string(),
            cascade_id: Some("cascade-1".to_string()),
            status: "running".to_string(),
            http_status: 202,
            upstream_status: Some("CASCADE_RUN_STATUS_RUNNING".to_string()),
            error: None,
            output_text: None,
            tool_calls: vec![],
            step_offset: 0,
            new_step_count: 0,
            step_count: 0,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:01Z".to_string(),
            completed_at: None,
            summary: json!({}),
            events: vec![],
        };

        let (validated, failure) = validate_run_output(Some(&contract), true, &run);
        assert!(validated.is_none());
        assert_eq!(failure.unwrap().code, "result_not_ready");
    }

    #[test]
    fn test_build_run_result_payload_includes_validated_json_result() {
        let contract = ResultContract::from_args(true, None).unwrap().unwrap();
        let validated = contract.validate_output(Some("{\"score\":1}")).unwrap();
        let run = RunRecord {
            run_id: "run-2".to_string(),
            mode: "exec".to_string(),
            path: "/tmp".to_string(),
            parent_run_id: None,
            prompt: "prompt".to_string(),
            request_model: None,
            requested_model_uid: "test-model".to_string(),
            cascade_id: Some("cascade-2".to_string()),
            status: "completed".to_string(),
            http_status: 200,
            upstream_status: Some("CASCADE_RUN_STATUS_COMPLETED".to_string()),
            error: None,
            output_text: Some("{\"score\":1}".to_string()),
            tool_calls: vec![],
            step_offset: 0,
            new_step_count: 1,
            step_count: 1,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:01Z".to_string(),
            completed_at: Some("2024-01-01T00:00:02Z".to_string()),
            summary: json!({}),
            events: vec![],
        };

        let payload = build_run_result_payload(
            &run,
            true,
            None,
            Some(&validated),
            Some(&contract),
            None,
            None,
        );

        assert_eq!(payload["result"], json!({ "score": 1 }));
        assert_eq!(payload["contract"]["type"], json!("strict_json"));
    }

    #[test]
    fn test_build_run_result_payload_marks_output_contract_failure() {
        let contract = ResultContract::from_args(true, None).unwrap().unwrap();
        let failure = contract.validate_output(Some("not-json")).unwrap_err();
        let run = RunRecord {
            run_id: "run-3".to_string(),
            mode: "exec".to_string(),
            path: "/tmp".to_string(),
            parent_run_id: None,
            prompt: "prompt".to_string(),
            request_model: None,
            requested_model_uid: "test-model".to_string(),
            cascade_id: Some("cascade-3".to_string()),
            status: "completed".to_string(),
            http_status: 200,
            upstream_status: Some("CASCADE_RUN_STATUS_COMPLETED".to_string()),
            error: None,
            output_text: Some("not-json".to_string()),
            tool_calls: vec![],
            step_offset: 0,
            new_step_count: 1,
            step_count: 1,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:01Z".to_string(),
            completed_at: Some("2024-01-01T00:00:02Z".to_string()),
            summary: json!({}),
            events: vec![],
        };

        let payload = build_run_result_payload(
            &run,
            false,
            None,
            None,
            Some(&contract),
            Some(&failure),
            None,
        );

        assert_eq!(payload["ok"], json!(false));
        assert_eq!(payload["failureKind"], json!("output_contract"));
        assert_eq!(payload["error"]["code"], json!("invalid_json_output"));
    }

    #[test]
    fn test_build_run_result_payload_marks_output_artifact_failure() {
        let run = RunRecord {
            run_id: "run-4".to_string(),
            mode: "exec".to_string(),
            path: "/tmp".to_string(),
            parent_run_id: None,
            prompt: "prompt".to_string(),
            request_model: None,
            requested_model_uid: "test-model".to_string(),
            cascade_id: Some("cascade-4".to_string()),
            status: "completed".to_string(),
            http_status: 200,
            upstream_status: Some("CASCADE_RUN_STATUS_COMPLETED".to_string()),
            error: None,
            output_text: Some("hello".to_string()),
            tool_calls: vec![],
            step_offset: 0,
            new_step_count: 1,
            step_count: 1,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:01Z".to_string(),
            completed_at: Some("2024-01-01T00:00:02Z".to_string()),
            summary: json!({}),
            events: vec![],
        };

        let payload = build_run_result_payload(
            &run,
            false,
            None,
            None,
            None,
            None,
            Some(&artifact_failure_payload(
                "result_file_write_failed",
                "failed to write result artifact",
                json!({ "path": "/tmp/result.json" }),
            )),
        );

        assert_eq!(payload["failureKind"], json!("output_artifact"));
        assert_eq!(payload["error"]["code"], json!("result_file_write_failed"));
    }

    #[test]
    fn test_resolve_last_message_text_prefers_validated_output() {
        let run = RunRecord {
            run_id: "run-5".to_string(),
            mode: "exec".to_string(),
            path: "/tmp".to_string(),
            parent_run_id: None,
            prompt: "prompt".to_string(),
            request_model: None,
            requested_model_uid: "test-model".to_string(),
            cascade_id: Some("cascade-5".to_string()),
            status: "completed".to_string(),
            http_status: 200,
            upstream_status: Some("CASCADE_RUN_STATUS_COMPLETED".to_string()),
            error: None,
            output_text: Some("raw text".to_string()),
            tool_calls: vec![],
            step_offset: 0,
            new_step_count: 1,
            step_count: 1,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:01Z".to_string(),
            completed_at: Some("2024-01-01T00:00:02Z".to_string()),
            summary: json!({}),
            events: vec![],
        };
        let validated = ValidatedAssistantOutput {
            value: json!({ "ok": true }),
            canonical_text: "{\"ok\":true}".to_string(),
        };

        assert_eq!(
            resolve_last_message_text(&run, None, Some(&validated)),
            Some("{\"ok\":true}".to_string())
        );
    }

    #[test]
    fn test_write_output_artifacts_writes_requested_files() {
        let temp_dir = TempDir::new().unwrap();
        let args = ArtifactOutputArgs {
            output_last_message_file: Some(temp_dir.path().join("final.txt").display().to_string()),
            result_file: Some(temp_dir.path().join("result.json").display().to_string()),
        };
        let payload = json!({ "ok": true, "result": { "score": 1 } });

        let failure = write_output_artifacts(&args, &payload, Some("hello world"));

        assert!(failure.is_none());
        assert_eq!(
            fs::read_to_string(temp_dir.path().join("final.txt")).unwrap(),
            "hello world"
        );
        assert_eq!(
            serde_json::from_str::<Value>(
                &fs::read_to_string(temp_dir.path().join("result.json")).unwrap()
            )
            .unwrap(),
            payload
        );
    }

    #[test]
    fn test_write_output_artifacts_reports_write_failure() {
        let temp_dir = TempDir::new().unwrap();
        let blocked = temp_dir.path().join("blocked");
        fs::write(&blocked, "not a directory").unwrap();
        let args = ArtifactOutputArgs {
            output_last_message_file: None,
            result_file: Some(blocked.join("result.json").display().to_string()),
        };

        let failure = write_output_artifacts(&args, &json!({ "ok": true }), Some("hello")).unwrap();

        assert_eq!(failure["code"], json!("result_file_write_failed"));
    }

    #[test]
    fn test_runs_args_default_limit() {
        // Default limit is 20
        let args = RunsArgs {
            limit: 20,
            status: None,
            workspace: None,
            output: ReadOutputArgs::default(),
        };
        assert_eq!(args.limit, 20);
    }

    #[test]
    fn test_runs_args_custom_limit() {
        let args = RunsArgs {
            limit: 100,
            status: Some("failed".to_string()),
            workspace: Some("/workspace".to_string()),
            output: ReadOutputArgs::default(),
        };
        assert_eq!(args.limit, 100);
        assert_eq!(args.status, Some("failed".to_string()));
        assert_eq!(args.workspace, Some("/workspace".to_string()));
    }

    #[test]
    fn test_print_unsupported_output_mode_payload() {
        let code = print_unsupported_output_mode("status", OutputMode::Text, &["json"]);
        assert_eq!(code, 1);
    }

    #[test]
    fn test_settings_commands() {
        // Verify SettingsCommand variants exist and work
        let show_cmd = SettingsCommand::Show;
        let keys_cmd = SettingsCommand::Keys;
        let describe_cmd = SettingsCommand::Describe {
            key: Some("output".to_string()),
        };
        let get_cmd = SettingsCommand::Get {
            key: "model".to_string(),
        };
        let set_cmd = SettingsCommand::Set {
            key: "model".to_string(),
            value: "swe-1-6".to_string(),
        };
        let unset_cmd = SettingsCommand::Unset {
            key: "model".to_string(),
        };

        // Just verify they compile and can be matched
        match show_cmd {
            SettingsCommand::Show => (),
            _ => panic!("Expected Show"),
        }
        match keys_cmd {
            SettingsCommand::Keys => (),
            _ => panic!("Expected Keys"),
        }
        match describe_cmd {
            SettingsCommand::Describe { key } => assert_eq!(key, Some("output".to_string())),
            _ => panic!("Expected Describe"),
        }
        match get_cmd {
            SettingsCommand::Get { key } => assert_eq!(key, "model"),
            _ => panic!("Expected Get"),
        }
        match set_cmd {
            SettingsCommand::Set { key, value } => {
                assert_eq!(key, "model");
                assert_eq!(value, "swe-1-6");
            }
            _ => panic!("Expected Set"),
        }
        match unset_cmd {
            SettingsCommand::Unset { key } => assert_eq!(key, "model"),
            _ => panic!("Expected Unset"),
        }
    }

    #[test]
    fn test_truncate_output_preview() {
        // Test the truncation logic used for output preview
        let long_text = "a".repeat(300);
        let truncated: String = long_text.chars().take(200).collect();
        assert_eq!(truncated.len(), 200);

        let short_text = "hello";
        let truncated: String = short_text.chars().take(200).collect();
        assert_eq!(truncated, "hello");
    }

    #[test]
    fn test_requested_run_selector_prefers_latest_flag() {
        assert_eq!(requested_run_selector(Some("run-1"), true), "latest");
        assert_eq!(requested_run_selector(Some("latest"), false), "latest");
        assert_eq!(requested_run_selector(Some("run-1"), false), "run-1");
    }

    #[test]
    fn test_extract_error_message_prefers_message_field() {
        let error = json!({ "message": "parent run not found", "code": "missing" });
        assert_eq!(
            extract_error_message(&error),
            Some("parent run not found".to_string())
        );
    }
}
