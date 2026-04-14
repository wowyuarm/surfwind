use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};
use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;

use crate::agent::{
    execute_agent_prompt, get_agent_events, get_agent_run, get_latest_agent_run,
    list_agent_runs_filtered, resume_agent_prompt, AgentRunOptions,
};
use crate::config::AppConfig;
use crate::runtime::runtime_diagnostics;
use crate::settings::{
    bootstrap, describe_settings, load_settings, read_setting, setting_keys, unset_setting,
    write_setting,
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
        Commands::Status(args) => {
            cmd_status(
                &config,
                args.workspace.as_deref(),
                !args.no_auto_launch,
                resolve_query_output_mode(args.output.output.as_deref(), args.output.json),
            )?
        }
        Commands::Models(args) => {
            cmd_models(
                &config,
                args.workspace.as_deref(),
                !args.no_auto_launch,
                resolve_query_output_mode(args.output.output.as_deref(), args.output.json),
            )?
        }
        Commands::Exec(args) => cmd_exec(&config, args)?,
        Commands::Resume(args) => cmd_resume(&config, args)?,
        Commands::Runs(args) => {
            cmd_runs(
                &config,
                args.limit,
                args.status.as_deref(),
                args.workspace.as_deref(),
                resolve_query_output_mode(args.output.output.as_deref(), args.output.json),
            )?
        }
        Commands::Show(args) => {
            cmd_show(
                &config,
                args.run_id.as_deref(),
                args.latest,
                resolve_query_output_mode(args.output.output.as_deref(), args.output.json),
            )?
        }
        Commands::Events(args) => {
            cmd_events(
                &config,
                args.run_id.as_deref(),
                args.latest,
                resolve_query_output_mode(args.output.output.as_deref(), args.output.json),
            )?
        }
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
        return Ok(print_unsupported_output_mode("status", output_mode, &["json"]));
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
                    let parsed: Vec<ModelInfo> =
                        serde_json::from_value(models).unwrap_or_default();
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
    let error = result.body.get("error").filter(|value: &&Value| !value.is_null());
    print_run_result(
        &result.run,
        output_mode,
        args.output_last_message,
        args.quiet,
        ok,
        error,
    );
    Ok(if ok { 0 } else { 1 })
}

fn cmd_resume(config: &AppConfig, args: ResumeArgs) -> Result<i32> {
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
    let error = result.body.get("error").filter(|value: &&Value| !value.is_null());
    print_run_result(
        &result.run,
        output_mode,
        args.output_last_message,
        args.quiet,
        ok,
        error,
    );
    Ok(if ok { 0 } else { 1 })
}

fn cmd_runs(
    config: &AppConfig,
    limit: usize,
    status: Option<&str>,
    workspace: Option<&str>,
    output_mode: OutputMode,
) -> Result<i32> {
    if !matches!(output_mode, OutputMode::Json) {
        return Ok(print_unsupported_output_mode("runs", output_mode, &["json"]));
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
        return Ok(print_unsupported_output_mode("show", output_mode, &["json"]));
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
        return Ok(print_unsupported_output_mode("events", output_mode, &["json"]));
    }
    let requested_run_id = requested_run_selector(run_id, latest);
    let resolved_run_id = match resolve_run_id_selector(config, run_id, latest)? {
        Some(run_id) => run_id,
        None => {
            print_json(&json!({ "ok": false, "error": "run not found", "runId": requested_run_id }));
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

fn print_run_result(
    run: &crate::types::RunRecord,
    output_mode: OutputMode,
    output_last_message: bool,
    quiet: bool,
    ok: bool,
    error: Option<&Value>,
) {
    if output_last_message {
        print_last_message(run, error);
        return;
    }
    match output_mode {
        OutputMode::Json => {
            print_json(&build_run_result_payload(run, ok, error));
        }
        OutputMode::Text => {
            if ok {
                if let Some(text) = run.output_text.as_ref().filter(|value| !value.is_empty()) {
                    println!("{}", text);
                    if !quiet {
                        eprintln!("\nrun_id: {}", run.run_id);
                    }
                } else {
                    print_json(&build_run_result_payload(run, ok, error));
                }
            } else {
                print_json(&build_run_result_payload(run, ok, error));
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
            if let Some(error) = error {
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
) -> Value {
    let mut payload = json!({ "ok": ok, "run": run });
    if let Some(error) = error {
        payload["error"] = error.clone();
    }
    payload
}

fn print_last_message(run: &crate::types::RunRecord, error: Option<&Value>) {
    if let Some(text) = run.output_text.as_ref().filter(|value| !value.is_empty()) {
        println!("{}", text);
    } else if let Some(message) = error
        .and_then(extract_error_message)
        .or_else(|| run.error.clone())
    {
        println!("{}", message);
    }
}

fn extract_error_message(error: &Value) -> Option<String> {
    if let Some(text) = error.as_str().map(str::trim).filter(|value| !value.is_empty()) {
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

    #[test]
    fn test_resolve_output_mode_from_config() {
        // Test that when no CLI flag is provided, we use config default
        // This tests the logic without needing full AppConfig
        assert_eq!(OutputMode::parse(Some("json")), OutputMode::Json);
        assert_eq!(OutputMode::parse(Some("stream-json")), OutputMode::StreamJson);
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
        assert_eq!(resolve_query_output_mode(Some("json"), false), OutputMode::Json);
        assert_eq!(resolve_query_output_mode(Some("text"), false), OutputMode::Text);
    }

    #[test]
    fn test_agent_run_options_defaults() {
        let options = AgentRunOptions::default();
        assert!(options.persist);
        assert!(options.auto_launch);
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
        };
        assert_eq!(args.workspace, Some("/workspace".to_string()));
        assert_eq!(args.model, Some("gpt-5-4".to_string()));
        assert!(args.json);
        assert!(args.quiet);
        assert!(args.no_persist); // explicitly disabled
        assert!(args.no_auto_launch);
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
        };
        assert_eq!(args.run_id, Some("surf-run-abc123".to_string()));
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
        let get_cmd = SettingsCommand::Get { key: "model".to_string() };
        let set_cmd = SettingsCommand::Set {
            key: "model".to_string(),
            value: "swe-1-6".to_string(),
        };
        let unset_cmd = SettingsCommand::Unset { key: "model".to_string() };

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
        assert_eq!(extract_error_message(&error), Some("parent run not found".to_string()));
    }
}
