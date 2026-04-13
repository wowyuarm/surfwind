---
name: surfwind-maintainer
description: Maintain and extend the surfwind repository itself. Use when an agent is changing the Rust codebase, updating documentation, preserving the headless runtime bridge architecture, refining agent-first CLI contracts, or deciding which files and invariants must move together in this repo.
---

Treat `surfwind` as an agent-first product, not a human-first shell UX project.

## Preserve the project position

- Keep `surfwind` focused on being the most reliable local CLI bridge to the existing Windsurf runtime.
- Prioritize script and orchestrator contracts before terminal ergonomics.
- Avoid adding UI-coupled behavior, HTTP server scope, or unrelated platform ambitions.

## Keep the main code boundaries straight

- `src/cli.rs`: command surface, output shaping, argument parsing.
- `src/agent.rs`: productized run execution, resume logic, normalized events, persistence decisions.
- `src/runtime.rs`: runtime discovery, headless bootstrap, port probing, runtime diagnostics.
- `src/settings.rs` and `src/config.rs`: defaults, settings persistence, environment overrides.
- `src/runstore.rs`: local run ledger.
- `src/types.rs`: shared CLI/runtime data contracts.

## Respect current product invariants

- The default workspace bootstrap path is headless and non-UI.
- `language_server` child bootstrap is the validated path for workspace attachment.
- Runtime side effects should stay explicit whenever possible.
- `exec` and `resume` are agent-facing contracts and should preserve stable structured outputs.
- Local run persistence is a product capability, but ephemeral execution must remain possible.

## Update docs as part of product work

- Update `README.md` when CLI-facing contracts or product positioning changes.
- Update `docs/architecture.md` when runtime, persistence, or module-boundary assumptions change.
- Update `SURFWIND_REVIEW.md` when implementation meaningfully changes the recommended priorities.
- Do not add long human-facing docs unless they materially improve future agent work.

## Prefer these maintenance habits

- Keep changes small and directly tied to the current product direction.
- Strengthen machine-readable behavior before adding convenience UX.
- Add tests when a new contract is introduced.
- When modifying a central abstraction, inspect both its CLI entrypoint and its persistence/output consumers.

## Validate before finishing

```bash
cargo test
```

Run additional targeted checks if a change affects CLI behavior, persistence, or runtime discovery.

## Read source documents deliberately

- Read `README.md` first for the public statement of what the product is.
- Read `docs/architecture.md` for runtime and persistence boundaries.
- Read `SURFWIND_REVIEW.md` for product-priority context.
- Read the code in `src/cli.rs`, `src/agent.rs`, and `src/runtime.rs` before making changes to any agent-facing contract.
