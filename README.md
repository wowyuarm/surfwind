# surfwind

`surfwind` is a Rust-only, headless-first CLI for driving the local Windsurf runtime.

## Product focus

- pure CLI
- headless workspace bootstrap
- local run persistence
- resume-capable cascade flows
- no default UI session hijacking
- no Python runtime dependency

## What it does

`surfwind` discovers or bootstraps a local Windsurf `language_server`, issues RPCs to it, and stores productized run records locally.

When a workspace is requested and no compatible runtime is already attached, it starts a dedicated headless `language_server` child for that repository using `--run_child`.

The default attach path does not use the current Windsurf UI session hook and does not rely on `remote-cli` window behavior.

## What it does not do

- no HTTP API server
- no `serve` command
- no `smoke` command
- no Python package entrypoint
- no default reuse of UI IPC hooks for workspace switching

## Build

```bash
cargo build
```

Run during development:

```bash
cargo run -- status
```

## Core commands

```bash
cargo run -- status
cargo run -- models
cargo run -- exec --workspace /path/to/repo "summarize this repository"
cargo run -- resume <run-id> "continue"
cargo run -- runs
cargo run -- show <run-id>
cargo run -- events <run-id>
cargo run -- settings show
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

## Status

This project currently targets the core CLI flow only. If an HTTP API is needed later, it should be implemented natively in Rust rather than migrated from the legacy Python server surface.
