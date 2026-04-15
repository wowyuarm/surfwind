---
name: surfwind
description: Drive the local Windsurf runtime through the surfwind CLI. Use when an agent needs to inspect runtime availability, list models, execute or resume prompts against a repository, inspect persisted runs and events, choose between text/json/stream-json output, target the latest run without pre-querying the ledger, or keep runtime side effects explicit in scripts and orchestrators.
---

Use `surfwind` as an agent-first CLI bridge, not as a human-oriented terminal UX.

## Keep the product model straight

- Treat `surfwind` as the reliable CLI entrypoint to the local Windsurf runtime already present on the machine.
- Prefer machine-readable contracts over pretty terminal output.
- Assume the caller may be another agent, a shell script, or an orchestrator.

## Start with the narrowest command that fits

1. Probe with `status --no-auto-launch` when you only need to know whether a compatible runtime already exists.
2. Use `models --no-auto-launch` when you need model discovery without bootstrapping a runtime.
3. Use `exec` to start a new run for a specific workspace.
4. Use `resume <run-id>` or `resume --last` when continuing a persisted run.
5. Use `runs`, `show`, and `events` only when you intentionally need the local run ledger.
6. Use `settings keys` and `settings describe <key>` when another agent needs to discover the stable settings contract.

## Choose commands by intent

- Use `status --no-auto-launch` to probe whether a compatible runtime is already available without creating side effects.
- Use `models --no-auto-launch` to inspect available models without bootstrapping a runtime.
- Use `exec` to start a new run.
- Use `resume <run-id>` to continue a persisted run.
- Use `runs`, `show`, and `events` only when you intentionally want to inspect the local run ledger.

## Choose run targeting deliberately

- Pass `--workspace /repo` to `exec` when the request is about a specific repository.
- Use `resume --last` when you want the latest resumable persisted run.
- Use `show latest` and `events latest` when the newest persisted run in general is the intended target.
- Use `runs --status <status>` and `runs --workspace /repo` before a resume only when you truly need filtering rather than the latest run shortcut.
- Do not expect `resume` to work for runs created with `--no-persist`.

## Choose output mode deliberately

- Prefer `--output stream-json` for automation.
- Expect `stream-json` to emit multiple JSONL `run.event` records followed by one final `run.result` record.
- Use `--json` or `--output json` when one structured object is easier for the caller to consume.
- Use text output only when the direct assistant text is the desired artifact.
- Use `--output-last-message` when a shell caller only needs the final assistant text instead of the full structured record.
- Use `--output-last-message-file <path>` to write the final assistant text to a file for downstream consumers.
- Use `--result-file <path>` to write the full structured JSON result to a file instead of relying on stdout parsing.

## Control side effects explicitly

- Pass `--no-auto-launch` when the caller wants discovery only and must not bootstrap a headless runtime implicitly.
- Pass `--no-persist` when the run is ephemeral and should not be written to the local run store.
- Remember that `--no-persist` trades away later `resume`, `show`, and `events` inspection.
- Pass `--timeout-seconds <int>` when the caller needs a hard deadline for the command rather than relying on internal polling limits.

## Enforce strict output contracts when needed

- Use `--strict-json` when the caller requires the final assistant output to be valid JSON; validation failures return `failureKind: output_contract`.
- Use `--output-schema <path>` when the caller requires the final output to match a specific JSON Schema file; the CLI validates at the boundary before returning success.
- Treat schema/JSON failures as contract violations (non-zero exit) rather than runtime failures, so callers can distinguish "agent produced wrong shape" from "runtime crashed".
- Combine `--strict-json` or `--output-schema` with `--result-file` when downstream tools expect both a validated structure and a deterministic file artifact.

## Handle results as agent contracts

- Treat exit status `0` as success for completed or still-running runs.
- Treat HTTP-style status `202` and normalized run status `running` as a normal long-running outcome, not as a fatal error.
- On failure, inspect both the top-level error object and the embedded run record.
- Read `run.summary`, `run.error`, and `run.events` before retrying blindly.
- Recognize structured failure taxonomy:
  - `timeout_reached`: command exceeded `--timeout-seconds` and was terminated
  - `output_contract` with `failureKind: output_contract`: final output failed strict JSON or schema validation
  - `output_artifact` with `failureKind: output_artifact`: artifact file could not be written

## Inspect the run ledger intentionally

- Use `runs` when you need a list view of persisted runs.
- Use `show <run-id>` or `show latest` when you need one run record.
- Use `events <run-id>` or `events latest` when you need normalized event history rather than the summary only.
- Prefer filtered ledger reads over broad history scans when another agent needs a specific failed or workspace-scoped run.

## Use practical calling patterns

```bash
surfwind status --no-auto-launch
surfwind models --no-auto-launch
surfwind exec --workspace /repo --output stream-json --no-persist "summarize this repo"
surfwind exec --workspace /repo --output-last-message "reply with the final answer only"
surfwind exec --workspace /repo --json "plan the next refactor"
surfwind exec --workspace /repo --timeout-seconds 120 --result-file ./artifacts/result.json --output-last-message-file ./artifacts/final.txt "solve within deadline"
surfwind exec --workspace /repo --strict-json --json "reply with a single JSON object"
surfwind exec --workspace /repo --output-schema ./result.schema.json --json "respond using the requested schema"
surfwind resume <run-id> --output stream-json "continue"
surfwind resume --last "continue"
surfwind runs --status failed --workspace /repo
surfwind show latest
surfwind events latest
surfwind settings keys
surfwind settings describe output
```

## Read source documents only when needed

- Read `README.md` for the public product positioning and supported CLI contracts.
- Read `docs/architecture.md` when runtime bootstrap or persistence behavior matters.

## Avoid common mistakes

- Do not treat `surfwind` as a generic terminal chatbot wrapper.
- Do not use ledger inspection commands when the caller only needs one fresh execution.
- Do not bootstrap a runtime implicitly if the caller asked for explicit side-effect control.
- Do not assume a `running` result means failure; it may be the expected long-running contract.
