# Browser 30-task sequence

Row B13 of the 0.2.0 bar. One command drives the whole loopback fixture
sequence through hand's public MCP-over-stdio endpoint.

```powershell
cd Q:\PROJECTS\PERSONAL\hand
node fixtures/run-tasks.mjs
```

Flags:

| Flag | Effect |
|---|---|
| `--only <id>` | run one task by id |
| `--repeat <n>` | run the whole sequence `n` times, sequentially |
| `--json` | emit one JSON result object instead of task lines |
| `--bin <path>` | copy a different hand executable (default `target/debug/hand.exe`) |

The runner copies the executable into a private temp directory before launch.
The main tree is rebuilt by another lane, and Windows will not replace a
running executable. The copy and its temp tree are removed when the run ends.

## Output

One line per task, then a run summary and an overall summary:

```
run 1/1  binary hand.exe sha256:515197715b2b
[ 1/30] PASS  t01-navigate-title
...
[30/30] PASS  t30-trace-har
summary run 1/1: pass=30 pending=0 fail=0
summary all runs: pass=30 pending=0 fail=0 exit=0
```

A pending line is a named absence, not a failure. Before row B3 landed, the
suite printed `[13/30] PEND  t13-network-mock  waits for browser_network(action=mock)`
and exited 0.

The exit code is 1 when any task is `fail` or the run aborts. `pending` does not
raise the exit code; it names the tool or capability the build lacks.

B13 receipt: `node fixtures/run-tasks.mjs --repeat 3` → `summary all runs:
pass=90 pending=0 fail=0`, exit 0 — three consecutive clean runs from one
binary (sha256:515197715b2b).

## Status contract

| Status | Meaning |
|---|---|
| `pass` | all tools exist and every predicate in `expect` held against live results |
| `pending` | a tool or tool capability in `requires` is absent from this build; the line names it |
| `fail` | the tools exist and at least one predicate did not hold |

A missing tool is never a pass and is never a silent skip. No task retries
itself, so a flaky pass stays visible across `--repeat` runs and across three
separate invocations.

Capability checks read the live `tools/list` schemas. A task that needs
`browser_network(action=mock)` is `pending` until that action appears in the
`action` enum. When the engine lane lands that change, the same
`fixtures/tasks.json` cells become runnable without a manifest edit.

## Data contract

`fixtures/tasks.json` is data, not code. Each task has `id`, `title`,
`requires`, ordered `calls` (`tool` plus `args`), `expect` (predicates over call
results), optional `bind` (lift a value out of an earlier result) and
`cleanup` (ordered tool calls). The runner owns the generic predicate and
placeholder evaluators; no task carries JavaScript.

Placeholders: `{{origin}}`, `{{originB}}`, `{{tempDir}}`, `{{uploadFile}}`, and
`{{<binding name>}}`.

## Fixture pages

The original benchmark and action pages are untouched. The task sequence uses
these additional loopback pages:

| Path | Purpose |
|---|---|
| `/tasks/main.html` | controls, same-process frame, cross-site OOPIF, shadow root, form, upload, download, drag ledgers, network button, clock |
| `/tasks/frame.html` | same-process iframe child |
| `/tasks/next.html` | second page for the stale-ref refusal |
| `/tasks/viewport.html` | overflow, low-contrast text, small focus target |
| `/tasks/network.json` | mockable network endpoint |
| `/tasks/payload.txt` | download payload |
| `/tasks/upload.txt` | upload payload |

The cross-site frame is served by the existing two-origin
`fixtures/interaction/serve.mjs` helper. The runner wires its `localhost` origin
into `main.html` at request time, so the OOPIF child is genuinely cross-site.

## Proof cadence

Measured on branch `feat/0.2.0` with three consecutive invocations:

| Run | pass | pending | fail | exit |
|---|---|---|---|---|
| 1 | 21 | 9 | 0 | 0 |
| 2 | 21 | 9 | 0 | 0 |
| 3 | 21 | 9 | 0 | 0 |

Each pending line names the tool it waits for. A pending task is neither a pass
nor a silent skip; a missing tool is never counted as a pass.

`node fixtures/validate.mjs` still covers the frozen 100-control benchmark
payload and its SHA256 contract. It is separate from this sequence.
