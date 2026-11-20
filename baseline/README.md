# hand compatibility baseline

Machine-readable capture of the two tool catalogues that `hand` must
reproduce, taken from the revisions the specification pins:

| Suite | Revision | Version | Tools | Spec prefix |
|---|---|---|---|---|
| Gladius | `59c6d77` | 0.8.0 | 25 | `browser_` |
| Puppet | `0f790be` | 8.0.0 | 7 | `desktop_` |

32 tools in total, which is the specification's first-release floor
([specification](../docs/SPEC.md), "Initial public surface and lifecycle").

The floor is what `hand` must serve; it is not the whole surface. The current
build publishes **38** tools — those 32 plus `browser_context`,
`browser_wheel`, `browser_drag`, `browser_form`, `browser_download` and
`browser_assert` — and `hand-catalog.json` records both
halves: `counts` is the derived floor, `live` is what the binary answered
`tools/list` with. Quoting `counts.total` as "the catalogue" is the mistake the
`live` block exists to prevent.

This directory captures catalog evidence and provides a compatibility gate. It
does not by itself prove the implementation's behavior or the complete suite.

## What is captured

`catalog/<suite>-<pin>.*` holds exactly what the server answered over stdio:

| File | Content |
|---|---|
| `*.tools.json` | the `tools/list` result: name, title, description, `inputSchema`, `outputSchema`, annotations |
| `*.tools.modern.json` | the same request with `_meta["io.modelcontextprotocol/protocolVersion"]="2026-07-28"`; differs from the legacy result only by `resultType` |
| `*.initialize.json` | protocol version, `serverInfo`, capabilities, `instructions` |
| `*.discover.json` | `server/discover` result |
| `*.ping.json` | `ping` result |
| `*.transcript.jsonl` | every frame sent and received, verbatim and in order |
| `*.meta.json` | receipt: executable path, size and SHA-256, `--version` output, server stderr, per-file hashes, canonical catalogue hash, timings |

Only `initialize`, `notifications/initialized`, `ping`, `tools/list` (twice)
and `server/discover` were sent. No tool was called, so no browser launched
and no desktop was touched.

`hand-catalog.json` is the derived baseline: both catalogues concatenated under
the documented `browser_` / `desktop_` prefix, with `legacyName` and `domain`
per tool. That field pair is the one-to-one migration mapping. Nothing else
changed: schemas, annotations, titles and descriptions are carried over
verbatim, so a difference between this file and a running server is a real
difference, not a normalization artifact. Its `blockers` and `notes` fields
record the derivation run: blockers are pinned-capture disagreements, notes are
tools captured beyond the required floor.

### The `live` block

`--serve` also asks the built server what it publishes, so the file stops
implying that 32 is everything:

| Field | Content |
|---|---|
| `counts.total` / `browser` / `desktop` | the derived floor, 32 = 25 + 7 pinned legacy tools |
| `counts.live` | how many tools the served binary published (38 today) |
| `live.names` | every name the server answered with, sorted |
| `live.extra` | the names above the floor, sorted (`browser_assert`, `browser_context`, `browser_download`, `browser_drag`, `browser_form`, `browser_wheel`) |
| `live.toolCount` | `live.names.length` |
| `live.protocolVersion` | the version the server negotiated for a `2025-11-25` request |
| `live.capture` | the executable path, its SHA-256, and the binary's own `--version` line |

That capture is provenance, not a log: `live.capture` carries exactly those
three keys — no timestamp, no environment, no build directory — so two runs
against the same binary produce the same bytes. A live name above the floor is
a note. A live block that contradicts itself is an `ISSUE` and exits 1: a count
that disagrees with its names, names out of order, a repeated name, a floor name
the server does not serve, an `extra` list that is not exactly the difference,
or an unexpected `capture` field.

Without `--serve` nothing about a running server is claimed: no `live` block, no
`counts.live`, no `live:` notes, and the derived file stays reproducible from
the captures alone. The two halves are separate facts for that reason — the
floor is derived and pinned, the live block is measured from one binary, and
that binary is identified only by its hash and version, never by a date.

## Why the capture is the pinned catalogue

The committed catalogues were captured from the release binaries already on
this machine, which stamp the commit they were built from. Three facts tie them
to the pins; `receipts/pin-differential.json` records the commands and outputs:

1. `git diff --stat <binary-stamp-commit> <pin> -- src Cargo.toml Cargo.lock build.rs`
   is empty for both suites: the binary's build inputs are byte-identical to
   the pin. (Gladius binary `d801b17` vs pin `59c6d77`; Puppet `6334393` vs
   `0f790be`.)
2. A fresh `cargo build --release --locked --offline` from `git archive <pin>`
   emits a canonically identical `tools/list` result: `bfbe2ee9…` for Gladius,
   `e417d102…` for Puppet, both matching the committed files.
3. Both archives and both binaries are hashed in `pins.json`.

No source repository was modified; `git archive` reads a commit without
touching the worktree, index or refs.

## Commands

```powershell
# integrity + comparator self-checks (20 tests, includes a live re-capture)
node --test baseline/test/

# derive the hand catalogue from the two captures (no live block: the floor only)
node baseline/scripts/build-hand-catalog.mjs

# derive it and record what a built server publishes: counts.live and live
node baseline/scripts/build-hand-catalog.mjs --serve Q:\path\to\hand.exe

# compare a candidate server against the hand baseline
node baseline/scripts/compare-catalog.mjs --serve Q:\path\to\hand.exe

# re-capture check: the pinned binary must still emit the committed catalogue
node baseline/scripts/compare-catalog.mjs --baseline gladius --pin 59c6d77 --serve Q:\PROJECTS\PERSONAL\gladius\target\release\gladius.exe
node baseline/scripts/compare-catalog.mjs --baseline puppet --pin 0f790be --serve Q:\PROJECTS\PERSONAL\puppet\target\release\puppet.exe

# re-capture the raw catalogues (rewrites catalog/)
node baseline/scripts/capture-mcp-catalog.mjs --cmd Q:\PROJECTS\PERSONAL\gladius\target\release\gladius.exe --out baseline/catalog/gladius-59c6d77
node baseline/scripts/capture-mcp-catalog.mjs --cmd Q:\PROJECTS\PERSONAL\puppet\target\release\puppet.exe --out baseline/catalog/puppet-0f790be
```

Exit codes: `compare-catalog.mjs` returns 0 pass, 1 mismatch, 2 usage or
transport error. `build-hand-catalog.mjs` returns 1 when a blocker or a live
block inconsistency is recorded and still writes the file, because the baseline
records what was captured rather than what was hoped for. A blocker is a
disagreement with the pinned capture: a tool count, version or canonical hash
that does not match `pins.json`, a required tool missing from the capture, or a
duplicate name. A tool captured *beyond* the required floor is a note instead —
printed as `NOTE`, recorded in `hand-catalog.json` under `notes`, and not an
exit-code change. It is not a blocker because the 32 names are a floor; extra
upstream capability does not make the capture wrong. The same rule covers a live
tool above the floor, while a live block that contradicts its own count, its
ordering or the floor is an `ISSUE` and exits 1. A live server that never
answers `tools/list` exits 2 and leaves the committed file untouched.

`compare-catalog.mjs` reads only the derived floor: `live` and `counts.live` are
provenance for what one binary publishes and are not part of the gate, which is
why an addition above the floor is reported as `extra` and passes.

The live tests skip rather than pass when a binary is absent; set
`HAND_BASELINE_SKIP_LIVE=1` to skip them deliberately.

## What the comparison treats as contract

| Field | `hand` profile (default) | `legacy` profile |
|---|---|---|
| name | must be `browser_<legacy>` / `desktop_<legacy>` | exact legacy name |
| `inputSchema`, `outputSchema`, `annotations` | structural equality, deep and key-order independent | structural equality |
| `title`, `description` | pinned; a `description` difference fails unless the tool is listed under `metadata.description` in `declared-additions.json`, and a listed description that still matches the pin fails as stale | failure |
| tool set | all 32 required names must be present; a missing one fails, an additional one is reported as `extra` and passes | exact set: a missing or an additional name fails |

A missing required tool, a duplicate name, a relaxed schema (for example a
dropped `required`), a changed output schema, or an unprefixed name all fail.
An additional tool is not a failure in the `hand` profile: the 32 names are a
floor, so a server that also exposes something new has it listed in `extra` and
still passes. In the `legacy` profile an additional tool does fail, because that
profile is a re-capture: it must reproduce the pinned catalogue exactly. The
tests prove both directions by breaking a copy of the baseline in each way, so
the gate cannot pass vacuously.

## Findings worth carrying into implementation

- **The two legacy suites collide on one tool name.** Gladius and Puppet both
  expose `screenshot`. The unprefixed union is therefore 31 unique names, not
  32, and `browser_screenshot` / `desktop_screenshot` are distinct tools with
  different schemas. The prefix is a correctness requirement, not a naming
  preference. Asserted in `test/catalog.test.mjs`.
- **Modern and legacy `tools/list` differ only by `resultType`.** Both servers
  answer the 2026-07-28 `_meta` request with an added `resultType: "complete"`
  and no other change; `catalog/*.tools.modern.json` records the exact result.
- **The advertised protocol version is `2025-11-25`** for both servers when the
  client asks for it, and both also answer `server/discover` with
  `supportedVersions`.
- **Gladius is headless-only and refuses `--headed` with exit 2**; the catalog
  is unaffected, but an acceptance scenario that assumes a visible browser
  window does not follow from these tests.

## Limits

- A catalogue is not behaviour. Schemas, descriptions and annotations are
  pinned; handlers, typed error codes, deadlines, budgets and no-steal
  invariants are not. Those are covered by each project's own test suites, and
  by the acceptance scenarios in the specification.
- 32 tools is the floor, not the product: the additional capabilities in the
  specification (isolated contexts, interception, clock/viewport control,
  private-desktop launch, semantic paging, visual-change waits, and the rest)
  are not in either catalogue and are not implied by this baseline. The `live`
  block records only what one build already adds — today `browser_context`,
  `browser_wheel`, `browser_drag`, `browser_form`, `browser_download` and
  `browser_assert` — and says nothing about the rest.
- The capture says nothing about whether either binary passes its own tests on
  this machine in this session.
- `ping`, `initialize` and `server/discover` results are captured for
  completeness; only `tools/list` is used as the compatibility contract.
- Windows x64 only, matching the pinned Launchers' distribution target.

`_work/` holds the throwaway extracted source trees and build outputs used for
the pin differential; it is gitignored and deleted after capture.
