# Checker contract (local adoption)

This repository vendors the phase-1 gate from the canonical "docs-template"
repository at cffb46a6c6f09ca452ffbf04e14c349fa65af814. That commit owns the
full CLI contract, including order-sensitive flags and repeated-flag rules;
this file lists only the supported standalone commands here and does not
restate those corner cases. The local pair is [docs-check.mjs](../docs-check.mjs)
and [docs-check.json](../docs-check.json). Tests live only in the canonical
template: no local test/ tree was vendored, so no local test path is implied.

## Standalone commands

From the repository root:

```text
node docs-check.mjs --report               # default: findings print, exit 0
node docs-check.mjs --strict               # exit 1 when findings exist
node docs-check.mjs --report --json        # stable JSON output
node docs-check.mjs --print-hash           # print the script SHA-256, exit 0
node docs-check.mjs --config <path> --report
```

`--config` defaults to docs-check.json in the current directory; config paths
resolve relative to that file. Exit codes are 0 for no findings or report mode,
1 for findings in strict mode, and 2 for a config or checker error. For any
flag combination or corner case, consult the canonical spec/checker.md in
docs-template at cffb46a6c6f09ca452ffbf04e14c349fa65af814.

## Scope

Phase 1 checks are forbidden, versions, generated and checker; link checking
is not in the Node tool (see [docs-structure.md](docs-structure.md) for the
PR-only report workflow). The checker hashes the script actually running and
normalizes line endings, so the targeted LF rule in .gitattributes keeps the
bytes identical across checkouts.
