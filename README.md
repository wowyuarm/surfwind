# surfwind

`surfwind` is an unofficial, Rust-only, agent-first CLI bridge for driving the local Windsurf runtime.

## Positioning

- primary consumer: agents, scripts, and orchestrators
- reuse local Windsurf runtime state when possible
- bootstrap a dedicated headless `language_server` for a workspace when needed
- keep runtime side effects and run persistence explicit
- stay pure CLI, Rust-only, and non-UI-coupled

This project is not trying to become a general human-first terminal UX layer. Its job is to be the most reliable local bridge between automation and the Windsurf runtime that already exists on the machine.

## Core behavior

`surfwind` discovers or bootstraps a local Windsurf `language_server`, issues RPCs to it, and stores productized run records locally.

When a workspace is requested and no compatible runtime is already attached, it starts a dedicated headless `language_server` child for that repository using `--run_child`.

The default attach path does not use the current Windsurf UI session hook and does not rely on `remote-cli` window behavior.

## Agent-first contracts

- `exec` and `resume` support `text`, `json`, and `stream-json` output
- `stream-json` emits normalized `run.event` JSONL records followed by a final `run.result`
- `exec` and `resume` support `--no-persist` for ephemeral runs
- `status`, `models`, `exec`, and `resume` support `--no-auto-launch` when the caller wants side effects to stay explicit
- long-running runs may finish polling as `running` with HTTP-style status `202`

## Core commands

```bash
cargo run -- status --no-auto-launch
cargo run -- models --no-auto-launch
cargo run -- exec --workspace /path/to/repo --output stream-json --no-persist "summarize this repository"
cargo run -- resume <run-id> "continue"
cargo run -- runs
cargo run -- show <run-id>
cargo run -- events <run-id>
cargo run -- settings show
```

## Non-goals

- no HTTP API server
- no `serve` command
- no `smoke` command
- no Python package entrypoint
- no default reuse of UI IPC hooks for workspace switching

## Build

```bash
cargo build
```

## Headless behavior

The workspace bootstrap path is intentionally non-UI:

- discover active `language_server` processes when they already exist
- otherwise spawn Windsurf's bundled `language_server_linux_x64`
- pass `--run_child`, `--random_port`, `--workspace_id`, and a generated CSRF token
- remove `VSCODE_IPC_HOOK_CLI` and related UI command variables from the child environment
- poll runtime discovery until the workspace-specific runtime appears

If a run is still executing when polling ends, the CLI preserves that as a `running` run with HTTP-style status `202` in the stored record, while still returning structured output to the caller.

## Local state

By default this project uses its own home directory:

```text
~/.surfwind/
  settings.json
  runs/
  logs/
```

Compatible environment variables such as `SURFWIND_HOME`, `SURFWIND_MODEL_UID`, `SURFWIND_METADATA_API_KEY`, `SURFWIND_LANGUAGE_SERVER_PATH`, and `SURFWIND_DATABASE_DIR` are still honored.
