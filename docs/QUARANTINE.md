# Test quarantine

Quarantine is the third option between a red release forever and skipping a test quietly.

A quarantined test is **reported, tracked and time-boxed**:

- **Reported** — the guard prints every entry (test, issue URL, age in days, re-arm condition) and CI runs the guard on every change, including docs-only changes. Nothing disappears from the output.
- **Tracked** — every ignored test in the guarded source dirs needs an entry in `.gates/quarantine.yaml` with `test`, `issue`, `added`, `owner` and `rearm_after_passes`. The issue URL carries the diagnosis.
- **Time-boxed** — `rearm_after_passes` is the exit condition. An entry is not a permanent exemption; its age is printed on every run so stale entries are visible.

The guard reads the source as evidence. An `#[ignore]` attribute under `crates/browser-engine/src` or `crates/hand/tests` without a matching ledger entry is a violation and exits 1. An entry with no matching `#[ignore]` in those dirs is stale: it is printed as a warning and still exits 0. An empty ledger with no `#[ignore]` attributes exits 0.

Scope note: hand is a root package, so its integration tests live in `tests/`, not `crates/hand/tests`. Three pre-existing `#[ignore]` attributes in `tests/` are outside the guard's scope for now; adding that dir requires ledger entries for them first.

## Who may add an entry

The maintainer who owns the failing test area, in the same change that adds the `#[ignore]`. The entry must point at a real tracking issue and name an owner responsible for re-arming. The guard validates all five fields: a missing field, a non-ISO `added` date, a non-URL `issue`, or a non-positive-integer `rearm_after_passes` exits 1 before any test is scanned.

No one may add an `#[ignore]` without an entry. The repository's own tests must not be annotated just to make a run green.

## What re-arms it

`rearm_after_passes` consecutive passing runs of that test, as shown by the flake-rate artifact. A pass is a run where the test executes and succeeds; an ignored or cancelled run does not count. The counter resets on any failure.

Removing an entry requires the test to pass `rearm_after_passes` consecutive runs as shown by the flake-rate artifact, then remove the `#[ignore]` attribute and the ledger entry in the same commit. Removing only the entry leaves an unledgered ignore and the guard exits 1; removing only the attribute leaves a stale entry that is warned about on every run.

## Run it

```sh
node scripts/quarantine-check.mjs
node --check scripts/quarantine-check.mjs
```

Exit 0 means clean or warning-only; exit 1 names the precise violation.
