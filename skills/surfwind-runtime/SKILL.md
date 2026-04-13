---
name: surfwind-runtime
description: Drive the local Windsurf runtime through the surfwind CLI. Use when an agent needs to inspect runtime availability, execute or resume prompts against a repository, choose between text/json/stream-json output, decide whether runs should persist, or keep runtime side effects explicit in scripts and orchestrators.
---

Use `surfwind` as an agent-first CLI bridge, not as a human-oriented terminal UX.

## Keep the product model straight

- Treat `surfwind` as the reliable CLI entrypoint to the local Windsurf runtime already present on the machine.
- Prefer machine-readable contracts over pretty terminal output.
- Assume the caller may be another agent, a shell script, or an orchestrator.

## Choose commands by intent

- Use `status --no-auto-launch` to probe whether a compatible runtime is already available without creating side effects.
- Use `models --no-auto-launch` to inspect available models without bootstrapping a runtime.
- Use `exec` to start a new run.
- Use `resume <run-id>` to continue a persisted run.
- Use `runs`, `show`, and `events` only when you intentionally want to inspect the local run ledger.

## Choose output mode deliberately

- Prefer `--output stream-json` for automation.
- Expect `stream-json` to emit multiple JSONL `run.event` records followed by one final `run.result` record.
- Use `--json` or `--output json` when one structured object is easier for the caller to consume.
- Use text output only when the direct assistant text is the desired artifact.

## Control side effects explicitly

- Pass `--no-auto-launch` when the caller wants discovery only and must not bootstrap a headless runtime implicitly.
- Pass `--no-persist` when the run is ephemeral and should not be written to the local run store.
- Do not expect `resume` to work for a run that was created with `--no-persist`.

## Handle results as agent contracts

- Treat exit status `0` as success for completed or still-running runs.
- Treat HTTP-style status `202` and normalized run status `running` as a normal long-running outcome, not as a fatal error.
- On failure, inspect both the top-level error object and the embedded run record.
- Read `run.summary`, `run.error`, and `run.events` before retrying blindly.

## Use practical calling patterns

```bash
surfwind status --no-auto-launch
surfwind models --no-auto-launch
surfwind exec --workspace /repo --output stream-json --no-persist "summarize this repo"
surfwind exec --workspace /repo --json "plan the next refactor"
surfwind resume <run-id> --output stream-json "continue"
```

## Read source documents only when needed

- Read `README.md` for the public product positioning and supported CLI contracts.
- Read `docs/architecture.md` when runtime bootstrap or persistence behavior matters.
- Read `SURFWIND_REVIEW.md` only when deciding future product priorities rather than executing the current CLI.
