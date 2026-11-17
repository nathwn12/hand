---
name: hand-mcp seed
description: Repo-specific commands, paths and ceremonies for hand-mcp. Load before working in this repo.
---

# hand-mcp — seed

Generated from this repo's manifests. `--check` fails when a declared command or path stops existing, so this file cannot rot silently.

<!-- seed:begin (generated from the manifests; edit outside the markers) -->

## Commands

| Task | Command |
|---|---|
| build | `cargo build --locked` |
| release | `cargo build --release --locked` |
| test | `cargo test` |
| lint | `cargo clippy --all-targets` |
| format | `cargo fmt --check` |

## Paths

- path: `README.md`
- path: `docs/SPEC.md`
- path: `docs/CAPABILITIES.md`
- path: `docs/COMPATIBILITY.md`
- path: `.opencode/skills`

## Notes

- cargo workspace: test and lint cover every member

<!-- seed:end -->

## Ceremonies

- Work one coherent slice at a time; each verified slice is its own commit.
- A mistake made once gets a guard (test, lint, script or pre-commit) and a row in the harness guard ledger.
- Report the command and the result; mark anything unverified as unverified.
- Never publish, push or release without the owner saying so.
