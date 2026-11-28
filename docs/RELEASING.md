# Releasing hand-mcp

This is the one canonical release guide. It has two audiences: the owner, who only ever uses a mouse and a passkey, and the agent, which runs the commands and reads the evidence. Command strings live in the [command reference](#for-the-agent-command-reference) exactly once; every later section refers to them by ID.

## Contents

- [For the owner: three things you do](#for-the-owner-three-things-you-do)
- [For the agent: command reference](#for-the-agent-command-reference)
- [The visible window contract](#the-visible-window-contract)
- [Case A: brand-new package](#case-a-brand-new-package)
- [Case B: existing package](#case-b-existing-package)
- [Case C: port this lane to another npm package](#case-c-port-this-lane-to-another-npm-package)
- [Recovery](#recovery)
- [Trusted publishing](#trusted-publishing)

## For the owner: three things you do

1. A PowerShell window opens on its own. You do not start it and you do not type into it.
2. Click the npm link printed in that window. If it asks you to press Enter to open the browser, press Enter.
3. Approve the sign-in with your passkey in 1Password.

Nothing is typed, no token is copied and no code is entered anywhere. After the one-time trusted-publisher binding you never do step 3 again for this package: routine releases publish from CI without you.

## For the agent: command reference

These are the only release commands. `<version>` means the current version in `npm/package.json`; `<gitHead>` means the commit the registry reports, never a locally guessed SHA.

| ID | Exact command | Run by | Use |
| --- | --- | --- | --- |
| O1 | `node npm/release.mjs --initial --visible` | agent | One-time npm web login with the passkey window. |
| O2 | `node npm/release.mjs --publish --visible` | agent | Local publish fallback; gate, build, pack, prove, publish, authoritative read-back. |
| O3 | `node npm/release.mjs --trust --visible` | agent | One-time GitHub Actions trusted-publisher binding. |
| A1 | `node npm/release.mjs --check` | agent | Read-only artifact proof, including HTTP navigation through the packaged launcher, before any publish. |
| A2 | `node npm/release.mjs --status` | agent | Read-only npm user, registry, tag and trust state. |
| A3 | `node npm/release.mjs --publish --ci` | CI only | OIDC publish with provenance; emits identity for the workflow's tag step. |
| V1 | `npm view hand-mcp@<version> gitHead --json` | agent | Confirm the version exists and which commit owns it. |
| V2 | `git ls-remote --tags origin refs/tags/v<version>*` | agent | Confirm the remote tag dereferences to that same commit. |
| V3 | `npm trust list hand-mcp` | agent | Confirm the trusted publisher names the repository and workflow. |
| V4 | `git tag v<version> <gitHead>` | agent | Create the tag locally only after V1 confirms the registry. |
| V5 | `git push origin refs/tags/v<version>` | agent | Push that immutable tag once. |

Modes and exit codes are fixed: `--check` and `--status` are read-only and exit 0 only when their proofs hold; a publish exits 0 for published or verified skip, 2 for PENDING (resumable), 1 for failure, 64 for bad flags. `--visible` is legal only with `--initial`, `--publish` and `--trust`; it is refused with `--check`, `--status` and `--ci`. When stdin is already a TTY the wrapper is unnecessary and the mode runs in place.

## The visible window contract

The parent derives the repo root from `import.meta.url` and uses a deterministic directory under `%TEMP%`: `hand-release-<mode>-<pid>` (mode is `initial`, `publish`, `trust` or `all`). It prints both paths before waiting, in this format:

```text
visible: window log %TEMP%/hand-release-<mode>-<pid>/child.log
visible: exit marker %TEMP%/hand-release-<mode>-<pid>/done.txt
visible: in that window the npm URL appears — click it, or press Enter if it asks to open the browser; approve with your passkey in 1Password. Nothing needs typing.
```

The generated `child.ps1` runs the same mode without `--visible`, with every stream teed to the log so the URL stays on screen:

```powershell
Set-Location -LiteralPath '<repo>'
& node '<repo>/npm/release.mjs' <mode flags> *>&1 | Tee-Object -FilePath '<log>'
$code = if ($null -eq $LASTEXITCODE) { 1 } else { $LASTEXITCODE }
$code | Set-Content -LiteralPath '<done>'
```

All paths are single-quoted with embedded quotes doubled. The parent then polls `done.txt` for at most 20 minutes, mirrors the child's exit code and prints `visible: log tail (N lines) from <log>:` followed by the last 20 log lines. If the deadline passes it reports the still-absent marker and the open window and exits non-zero; it never kills the owner's window. Do not run a `--visible` mode unattended if the outcome depends on a passkey you cannot supply.

## Case A: brand-new package

Nothing is on the registry and no publisher is bound yet.

| Step | ID | Mode | Exit | Print to the owner | Wait on | Verify afterwards |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | O1 | `--initial --visible` | 0, or skip when already logged in | The window is open; click the npm link or press Enter, then approve with the passkey. | `done.txt`, then the log tail | A2 shows an npm user. |
| 2 | A1 | `--check` | 0 | Nothing: this step is read-only. | Process exit | Tarball name, byte count, sha256 and the packed file list are all printed. |
| 3 | O2 | `--publish --visible` | 0 published or skip, 2 PENDING | Same as step 1; the URL may appear again at publish. | `done.txt`, then the authoritative read-back | V1 and V2 agree on the same commit. |
| 4 | O3 | `--trust --visible` | 0 | Same as step 1 for the bind. | `done.txt`, then the trust listing | V3 names the repository and workflow. |

A first publish cannot use OIDC: npm has no package to bind a trusted publisher to until that package exists. Publish once with O2, then bind with O3; from then on CI can publish with OIDC. The tag is created only after the registry confirms the version: V1 must report the same commit as the built HEAD before V4 and V5 run. A tag written before that confirmation can never be moved onto a different published source.

## Case B: existing package

1. Bump the version in `npm/package.json` and the root `Cargo.toml` together, add the CHANGELOG entry, commit and push `main`. The preflight refuses a version disagreement, a dirty tree, a non-main branch or an unpushed HEAD.
2. CI runs A3 on the push: it publishes over OIDC with `--provenance` and tags the registry's reported gitHead in its own step. The owner does nothing.
3. Use O2 only when CI is unavailable. That fallback does not tag: confirm with V1, then run V4 and V5 yourself.
4. Verify with V1, V2 and V3. A2 is the quick read-only summary; a `needs-passkey` trust finding there is a finding, not a failure.

A published version is immutable. If a number is burned by a bad publish or a conflicting tag, do not republish or retag it: bump to a new number and release that instead.

## Case C: port this lane to another npm package

1. Copy `npm/release.mjs` into the new repository's `npm/` directory; keep it as the only release driver.
2. Set the workflow file name to the actual file under `.github/workflows/`. `GITHUB_REPO` is derived from `package.json.repository` and falls back to `nathwn12/hand`, so a copy with a real repository URL needs no edit there.
3. Parameterise what is package-specific rather than hard-coding it: package name and version (already read from `package.json`), repository owner/name (from `repository.url`), workflow file name, tarball launcher path, the baseline tool-name floor, the registry URL, the temp-directory prefix, the publish access flag and the `v` tag prefix. The preflight still requires the Cargo version to match the package version; a package without a Cargo manifest needs that check replaced, not bypassed.
4. Run O1, then A1, then the first publish with O2, then O3 in that order.
5. Verify with V1, V2 and V3, substituting the new package name in V1 and V3.

## Recovery

| Symptom | Meaning | Action |
| --- | --- | --- |
| Exit 2 `PENDING` | Publish was accepted but not visible through the authoritative read within 180 s. | Wait, then rerun the publish: it resumes as a verified skip and tags the registry's gitHead. Never republish. |
| Identity conflict | The tag and registry (or the published gitHead and current HEAD) disagree. | Stop. The tag or the registry owns that version; do not retag or republish. Bump to a new number if the release is unusable. |
| Dirty tree, not on main, not pushed | Publish preflight refuses to build an unreviewed candidate. | Commit or set aside your own changes, check out `main`, push, rerun. A1 tolerates local changes because it never writes. |
| Registry unavailable | A2 reports `registry ... unavailable`; publish fails closed. | Retry when reachable. Never treat an unavailable registry as an absent version. |
| Passkey window closed | `done.txt` holds a non-zero code and the log tail names the auth step. | Read the log tail, reopen the mode with `--visible` and finish the approval. Nothing was typed into the closed window. |
| Version already published from this commit | Skip is success, not an error. | Let the skip emit the release identity and tag only from the published gitHead. |

## Trusted publishing

`publish.yml` needs `contents: write` for the tag step and `id-token: write` on the publishing job; its publish step runs A3, which adds `--provenance`. No token, OTP or manual publish belongs in that workflow.

The bind is one-time: O3 registers `publish.yml` and `nathwn12/hand` as the trusted publisher for the package. Verify it with V3: the listing must name both the repository and the workflow file. If V3 asks for a passkey, A2 reports `trust: needs-passkey` as a finding and still exits 0.

Trust breaks if the workflow file is renamed, the repository moves, or the workflow path in the binding no longer matches the file that runs. Restore the name and path or redo the one-time bind with O3; a signing failure in CI usually means this binding, not the build.
