# surfwind

`surfwind` is an unofficial, non-interactive agent-first Rust CLI bridge for driving the local Windsurf runtime.

## What is surfwind?

`surfwind` is a non-interactive CLI for driving the local Windsurf runtime. Unlike general-purpose agent CLIs like `claude -p` or `codex exec`, it is specifically designed to:

- Bridge directly to Windsurf's `language_server` for local model execution
- Reuse existing Windsurf runtime state when possible
- Provide workspace-scoped execution with explicit safety boundaries
- Maintain a durable local run ledger for audit, resume, and debugging

Think of it as the most reliable way for automation and orchestration systems to interact with the Windsurf runtime already installed on your machine.

## Quick Start

```bash
# Check runtime status
surfwind status

# List available models
surfwind models

# Run a simple prompt
surfwind exec --workspace /path/to/repo "summarize this codebase"

# Get structured output
surfwind exec --workspace /path/to/repo --output stream-json "explain the main function"
```

## Key Features

- **Multiple output formats** — `text`, `json`, or `stream-json` (JSONL events) for different integration needs
- **Structured output** — JSON Schema validation with `--output-schema`
- **Artifact extraction** — Save final messages and structured results to files
- **Session management** — Resume runs, query history, filter by status/workspace
- **Explicit control** — `--no-auto-launch` and `--no-persist` for predictable automation
- **Non-interactive** — No TUI, no prompts—built for scripts and CI/CD pipelines

## Installation

### From source

```bash
# Clone the repository
git clone https://github.com/wowyuarm/surfwind.git
cd surfwind

# Build and install locally
cargo install --path .

# Or build without installing
cargo build
# The binary will be at target/release/surfwind
```

## Architecture

The workspace bootstrap path is intentionally non-UI:

- discover active `language_server` processes when they already exist
- otherwise spawn Windsurf's bundled `language_server_linux_x64`
- pass `--run_child`, `--random_port`, `--workspace_id`, and a generated CSRF token
- remove `VSCODE_IPC_HOOK_CLI` and related UI command variables from the child environment
- poll runtime discovery until the workspace-specific runtime appears

If a run is still executing when polling ends, the CLI preserves that as a `running` run with HTTP-style status `202` in the stored record, while still returning structured output to the caller.

## Non-goals

- HTTP API server or `serve` command
- Interactive TUI or human-first terminal UX
- Python package entrypoint
- General-purpose agent framework (not competing with `claude -p` or `codex exec`)
- Cloud execution or remote runtime management

## Local state

By default this project uses its own home directory:

```text
~/.surfwind/
  settings.json
  runs/
  logs/
```

Environment variables: `SURFWIND_HOME`, `SURFWIND_MODEL_UID`, `SURFWIND_METADATA_API_KEY`, `SURFWIND_LANGUAGE_SERVER_PATH`, `SURFWIND_DATABASE_DIR`
