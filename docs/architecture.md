# Architecture

## Goal

Define the minimal durable architecture for a Rust-only `surfwind` successor that preserves the validated headless attachment path and avoids the historical UI-coupled failure modes.

## Core decision

The default workspace bootstrap backend is a direct Windsurf `language_server` child process.

The system should not use:

- the current session's `VSCODE_IPC_HOOK_CLI`
- `remote-cli` window semantics
- implicit UI retargeting as a fallback

## Runtime model

The runtime layer has four responsibilities:

- resolve a requested workspace to a repo root
- discover existing `language_server` processes and their ports
- validate the correct metadata API key by probing `GetUserStatus`
- spawn a new headless child when the workspace is not attached yet

The headless spawn contract is:

- binary: Windsurf bundled `language_server_linux_x64`
- flags: `--run_child`, `--random_port`, `--workspace_id`
- environment: generated `WINDSURF_CSRF_TOKEN`
- isolation: remove UI IPC and client-command variables before spawn
- current directory: workspace root

## Agent model

The agent layer translates runtime RPCs into product-level runs.

Execution flow:

1. validate prompt and workspace
2. discover or bootstrap runtime
3. choose a working port with `GetUserStatus`
4. trust workspace with `UpdateWorkspaceTrust`
5. create or reuse cascade
6. send user message
7. poll `GetCascadeTrajectorySteps` and `GetCascadeTrajectory`
8. store a normalized run record

The agent-facing CLI contract should stay explicit about:

- output mode selection for automation, including `stream-json`
- minimal final-text extraction for shell callers via `--output-last-message`
- whether a call persists into the local run ledger
- whether a command may auto-launch a headless runtime as a side effect
- how callers target the latest persisted run and filter ledger reads without extra client-side plumbing

## Persistence model

Each run is written as one JSON file in the local run store.

A run record preserves:

- run identity
- requested model
- cascade id
- workspace identity
- productized events
- assistant output
- tool call envelopes
- upstream status
- normalized final status

## CLI boundary

The current product boundary is intentionally narrow:

- `status`
- `models`
- `exec`
- `resume`
- `runs`
- `show`
- `events`
- `settings`

The boundary is optimized for agent/script callers first, not for human terminal UX first.

No HTTP server is part of the current target architecture.

## Immediate refactor direction

The runtime and agent layers are now split into focused submodules:

- `runtime/discovery.rs`
- `runtime/rpc.rs`
- `runtime/headless.rs`
- `agent/execute.rs`
- `agent/poll.rs`
- `agent/events.rs`

The next cleanup step should keep those boundaries crisp and avoid letting `execute.rs` or `poll.rs` become new catch-all files. In particular, CLI-facing ergonomics such as latest-run resolution, ledger filtering, and settings discoverability should continue to live at the CLI/query edges rather than leaking into runtime bootstrap logic.

That refactor is structural only and should not change the validated headless attach contract.
