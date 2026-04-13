use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use serde_json::json;
use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;

use crate::agent::{
    execute_agent_prompt, get_agent_events, get_agent_run, list_agent_runs, resume_agent_prompt,
};
use crate::config::AppConfig;
use crate::runtime::runtime_diagnostics;
use crate::settings::{bootstrap, load_settings, read_setting, unset_setting, write_setting};
use crate::types::OutputMode;

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

#[derive(Args, Debug)]
struct StatusArgs {
    #[arg(long)]
    workspace: Option<String>,
}

#[derive(Args, Debug)]
struct RunsArgs {
    #[arg(long, default_value_t = 20)]
    limit: usize,
}

#[derive(Args, Debug)]
struct RunIdArgs {
    run_id: String,
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
    #[arg(short = 'q', long)]
    quiet: bool,
}

#[derive(Args, Debug)]
struct ResumeArgs {
    run_id: String,
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
    #[arg(short = 'q', long)]
    quiet: bool,
}

#[derive(Args, Debug)]
struct SettingsArgs {
    #[command(subcommand)]
    command: SettingsCommand,
}

#[derive(Subcommand, Debug)]
enum SettingsCommand {
    Show,
    Get { key: String },
    Set { key: String, value: String },
    Unset { key: String },
}

pub fn run() -> Result<i32> {
    let cli = Cli::parse();
    let config = AppConfig::load()?;
    bootstrap(&config.paths)?;

    let code = match cli.command {
        Commands::Status(args) => cmd_status(&config, args.workspace.as_deref())?,
        Commands::Models(args) => cmd_models(&config, args.workspace.as_deref())?,
        Commands::Exec(args) => cmd_exec(&config, args)?,
        Commands::Resume(args) => cmd_resume(&config, args)?,
        Commands::Runs(args) => cmd_runs(&config, args.limit)?,
        Commands::Show(args) => cmd_show(&config, &args.run_id)?,
        Commands::Events(args) => cmd_events(&config, &args.run_id)?,
        Commands::Settings(args) => cmd_settings(&config, args.command)?,
    };
    Ok(code)
}

fn cmd_status(config: &AppConfig, workspace: Option<&str>) -> Result<i32> {
    match runtime_diagnostics(config, workspace) {
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

fn cmd_models(config: &AppConfig, workspace: Option<&str>) -> Result<i32> {
    match runtime_diagnostics(config, workspace) {
        Ok(body) => {
            let models = body.get("models").cloned().unwrap_or_else(|| json!([]));
            print_json(&json!({ "ok": true, "models": models }));
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
    let result = execute_agent_prompt(
        config,
        &prompt,
        args.model.as_deref(),
        args.workspace.as_deref(),
    );
    let output_mode = resolve_output_mode(config, args.output.as_deref(), args.json);
    if matches!(result.status, 200 | 202) {
        print_run_output(&result.run, output_mode, args.quiet);
        Ok(0)
    } else {
        print_json(&json!({ "ok": false, "error": result.body.get("error"), "run": result.run }));
        Ok(1)
    }
}

fn cmd_resume(config: &AppConfig, args: ResumeArgs) -> Result<i32> {
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
    let result = resume_agent_prompt(
        config,
        &args.run_id,
        &prompt,
        args.model.as_deref(),
        args.workspace.as_deref(),
    );
    let output_mode = resolve_output_mode(config, args.output.as_deref(), args.json);
    if matches!(result.status, 200 | 202) {
        print_run_output(&result.run, output_mode, args.quiet);
        Ok(0)
    } else {
        print_json(&json!({ "ok": false, "error": result.body.get("error"), "run": result.run }));
        Ok(1)
    }
}

fn cmd_runs(config: &AppConfig, limit: usize) -> Result<i32> {
    let runs = list_agent_runs(config, limit)?;
    print_json(&json!({ "ok": true, "runs": runs }));
    Ok(0)
}

fn cmd_show(config: &AppConfig, run_id: &str) -> Result<i32> {
    if let Some(run) = get_agent_run(config, run_id)? {
        print_json(&json!({ "ok": true, "run": run }));
        Ok(0)
    } else {
        print_json(&json!({ "ok": false, "error": "run not found", "runId": run_id }));
        Ok(1)
    }
}

fn cmd_events(config: &AppConfig, run_id: &str) -> Result<i32> {
    if let Some(events) = get_agent_events(config, run_id)? {
        print_json(&json!({ "ok": true, "runId": run_id, "events": events }));
        Ok(0)
    } else {
        print_json(&json!({ "ok": false, "error": "run not found", "runId": run_id }));
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

fn print_run_output(run: &crate::types::RunRecord, output_mode: OutputMode, quiet: bool) {
    match output_mode {
        OutputMode::Json => {
            print_json(&json!({ "ok": true, "run": run }));
        }
        OutputMode::Text => {
            if let Some(text) = run.output_text.as_ref().filter(|value| !value.is_empty()) {
                println!("{}", text);
                if !quiet {
                    eprintln!("\nrun_id: {}", run.run_id);
                }
            } else {
                print_json(&json!({ "ok": true, "run": run }));
            }
        }
    }
}

fn print_json(value: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
    );
}
