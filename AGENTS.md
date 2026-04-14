# Repository Guidelines

## Project Overview
surfwind is a Rust-only, agent-first CLI bridge to the local Windsurf runtime.
It discovers or bootstraps a headless `language_server`, executes and resumes
runs, and persists normalized records for scripts and orchestrators.
The product scope is intentionally narrow: reliable automation contracts,
explicit side effects, and no UI-coupled behavior.

## Repository Structure
- `src/`: primary Rust source code.
- `src/agent/`: run execution, polling, and event normalization.
- `src/runtime/`: runtime discovery, headless bootstrap, and RPC calls.
- `tests/`: integration tests and shared test helpers.
- `docs/`: architecture notes and durable design decisions.
- `skills/`: agent skill cards used by automation workflows.
- `README.md`: public CLI contract, examples, and non-goals.
- `SURFWIND_REVIEW.md`: product-priority and maintenance context.

## Build & Development Commands
```bash
# build
cargo build

# test
cargo test
cargo test --test integration_tests

# lint / format / type-check
cargo fmt --all
cargo clippy --all-targets --all-features
cargo check

# run locally
cargo run -- status --no-auto-launch
cargo run -- models --no-auto-launch
cargo run -- exec --workspace /path/to/repo --output stream-json --no-persist "summarize this repository"
cargo run -- resume --last "continue"
```
> TODO: Add project-specific debug and deploy commands when standardized.

## Code Style & Conventions
- Follow rustfmt defaults (4-space indentation, standard Rust style).
- Use `snake_case` for functions/modules/files and `PascalCase` for types.
- Keep responsibilities separated: CLI in `src/cli.rs`, runtime in `src/runtime/`,
  orchestration in `src/agent/`.
- Preserve agent-facing output contracts (`text`, `json`, `stream-json`).
- Commit message template: `<type>: <imperative summary>`.
  Common types in this repo include `feat`, `fix`, and `refactor`.

## Architecture Notes
```text
Caller/Script
   |
   v
src/cli.rs  -->  src/agent/*  -->  src/runtime/*  --> Windsurf language_server
   |                  |
   |                  v
   +------------> src/runstore.rs (persisted run JSON records)
```
The CLI validates inputs and shapes output, the agent layer orchestrates run
lifecycle and normalization, and the runtime layer handles process discovery,
attachment, and RPC communication.

## Testing Strategy
- Tests use Rust `#[test]` with `cargo test` as the primary entrypoint.
- Cross-module behavior and contract tests live in `tests/integration_tests.rs`.
- Run local full checks before PRs: `cargo test` and `cargo clippy --all-targets`.
- > TODO: Add end-to-end test workflow and CI matrix documentation.

## Security & Compliance
- Never hardcode secrets; use env vars such as `SURFWIND_METADATA_API_KEY`.
- Respect explicit side-effect controls like `--no-auto-launch` and `--no-persist`.
- Validate workspace paths and runtime inputs before changing bootstrap behavior.
- License is `UNLICENSED` (see `Cargo.toml`).
- > TODO: Document dependency scanning command/tooling.

## Agent Guardrails
1. Do not add UI-coupled runtime fallback behavior.
2. Do not introduce HTTP server scope or unrelated platform features.
3. Keep `exec` and `resume` contracts stable for automation clients.
4. Update `README.md` and `docs/architecture.md` with contract or boundary changes.
5. Require human review for major changes under `src/runtime/` and `src/agent/`.

## Extensibility Hooks
- Environment-based hooks: `SURFWIND_HOME`, `SURFWIND_MODEL_UID`,
  `SURFWIND_METADATA_API_KEY`, `SURFWIND_LANGUAGE_SERVER_PATH`,
  `SURFWIND_DATABASE_DIR`.
- CLI output modes (`text`, `json`, `stream-json`) are stable integration hooks.
- > TODO: Document feature flags or plugin interfaces if introduced.

## Further Reading
- `README.md`
- `docs/architecture.md`
- `SURFWIND_REVIEW.md`
- `skills/surfwind/SKILL.md`
