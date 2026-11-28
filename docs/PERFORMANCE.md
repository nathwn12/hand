# Performance

Measured on 2026-11-28 with the 0.4.0 release build. Every timing below is a wall-clock
reading taken by benchmarks/perf/run.mjs (Node process.hrtime.bigint()), every size is
a byte count from a real file or a real protocol frame, and nothing is estimated.
Reproduce with "node benchmarks/perf/run.mjs" (optionally --exe <path> --reps N --json).

## Method

- Repetitions. Every timing row is a median of 10 independent runs; min and max are the
  observed extremes. Size rows are single readings of real files (no timing noise).
- Cold browser. A fresh server process for every repetition: start hand mcp, initialize,
  tools/list, then time the first browser_navigate to a loopback fixture served by the
  harness. The pinned per-user chrome-headless-shell cache is left exactly as found; the
  harness never deletes, replaces or re-provisions it. One cold repetition in an earlier
  pass hit the engine's known first-load race and retried once; the reported time is first
  attempt through first successful load, and the attempt count is a row in the table.
- Warm browser. Calls made after that first successful navigation in the same server
  process: a second browser_navigate, then browser_snapshot, browser_click on a ref from
  the live snapshot, browser_screenshot (png, full page), browser_evaluate,
  browser_metrics, and a browser_run batch of 10 steps (1 navigate + 9 evaluates).
- Handshake. Spawn to the tools/list response frame, including initialize and the
  notifications/initialized notification. The response size is the raw newline-delimited
  JSON frame in bytes.
- Launcher. Spawn-to-exit wall time of node npm/bin/hand-mcp.js --version.
- Sizes. Release hand.exe by stat; a real npm pack into a temp directory from a temp copy
  of the package with this build staged there (the owned npm/ tree is never written);
  unpacked size is the sum of the tarball's file entries. All temp files are removed.
- Desktop. The largest visible, uncloaked, non-minimized top-level window (here
  explorer.exe, 2560x1440) is the target. desktop_record asks for 1000 ms at 8 fps.
  frames counts every AVI chunk written (fresh captures plus repeats of the previous
  frame); capturedFrames counts fresh captures; droppedFrames counts overrun slots.
  Achieved fps is fresh captures per recorded second, AVI fps is written chunks per
  recorded second; both come from the tool's own counts, not the requested fps.

## Machine

| | |
|---|---|
| CPU | AMD Ryzen 7 5800X, 8 physical / 16 logical cores |
| RAM | 31.9 GiB (34,276,126,720 bytes reported) |
| OS | Windows 11 Pro, build 26300, x64 |
| Date | 2026-11-28 |
| Toolchain | Node v26.13.1, npm 12.2.3, cargo/rustc 1.100.1 |
| Binary | target/release/hand.exe, 19,881,984 bytes |

## 0.4.0 results (run C, 2026-11-28T08:21:35Z)

Timings are ms; units for the other rows are in the Metric column. Reps is 10 for all
measurement rows and 1 for the three size rows.

| Metric | Unit | Median | Min | Max | Reps | Detail |
|---|---|---:|---:|---:|---:|---|
| startup --version | ms | 13.73 | 12.88 | 17.70 | 10 | hand 0.4.0 |
| handshake to tools/list | ms | 24.46 | 22.35 | 30.32 | 10 | 44 tools |
| tools/list response frame | bytes | 90,943 | 90,943 | 90,943 | 10 | newline-delimited JSON |
| browser_navigate cold | ms | 388.97 | 365.51 | 742.19 | 10 | first browser call in a fresh server |
| browser_navigate warm | ms | 27.73 | 17.06 | 34.35 | 10 | same process, browser already up |
| browser_snapshot | ms | 6.84 | 4.86 | 11.23 | 10 | |
| browser_click | ms | 133.81 | 125.75 | 135.81 | 10 | ref from the live snapshot |
| browser_screenshot | ms | 128.44 | 95.58 | 151.14 | 10 | png, full page |
| browser_evaluate | ms | 0.56 | 0.45 | 1.19 | 10 | |
| browser_metrics | ms | 1.87 | 1.48 | 2.36 | 10 | |
| browser_run 10 steps | ms | 20.74 | 19.08 | 26.33 | 10 | 1 navigate + 9 evaluates, end to end |
| cold navigate attempts | tries | 1 | 1 | 1 | 10 | one earlier pass saw a 2-try cold load |
| snapshot nodes | nodes | 5 | 5 | 5 | 10 | loopback fixture accessibility tree |
| browser_screenshot png | bytes | 10,691 | 10,691 | 10,691 | 10 | 1440x900 viewport |
| browser_run reported ms | ms | 20 | 18 | 25 | 10 | server-side aggregate |
| browser_run steps | steps | 10 | 10 | 10 | 10 | all ok |
| desktop_list_windows | ms | 3.01 | 2.69 | 7.71 | 10 | |
| visible windows | windows | 16 | 16 | 16 | 10 | reported window count |
| desktop_screenshot | ms | 72.50 | 67.11 | 80.09 | 10 | target explorer.exe |
| desktop_screenshot png | bytes | 18,781 | 18,781 | 18,781 | 10 | decoded image/png frame |
| desktop_record | ms | 955.69 | 948.86 | 970.17 | 10 | 1000 ms requested, 8 fps |
| record frames written | frames | 8 | 8 | 8 | 10 | AVI chunks (fresh + repeats) |
| record fresh captures | frames | 4 | 4 | 4 | 10 | capturedFrames |
| record dropped slots | frames | 4 | 4 | 4 | 10 | droppedFrames |
| record achieved fps | fps | 4 | 4 | 4 | 10 | fresh captures per recorded second |
| record AVI fps | fps | 8 | 8 | 8 | 10 | written chunks per recorded second |
| record AVI size | bytes | 90,280 | 90,280 | 90,280 | 10 | file on disk |
| launcher node --version | ms | 74.53 | 70.75 | 84.95 | 10 | staged launcher binary reported 0.4.0 |
| release exe | bytes | 19,881,984 | - | - | 1 | stat of the built binary |
| npm tarball | bytes | 6,611,734 | - | - | 1 | hand-mcp-0.4.0.tgz, real npm pack |
| installed unpacked | bytes | 19,890,344 | - | - | 1 | 5 files in the tarball |

npx -y hand-mcp --version was NOT MEASURED: no hand-mcp package is present in the local
npm cache, and an offline probe exited ENOTCACHED ("cache mode is only-if-cached but no
cached response is available"). A network fetch would be required, so no number is
reported for it.

## Comparison with 0.3.3

The same two measurements, in the same run C, against
C:/Users/nathan/.local/share/hand-mcp-0.3.3/node_modules/hand-mcp/hand.exe
(hand 0.3.3 (2e8b48b5b42d0d5958fab6871d4f9e5369718292; release)).

| Metric | 0.3.3 median | 0.3.3 min-max | 0.4.0 median | Change |
|---|---:|---:|---:|---|
| startup --version | 12.92 ms | 11.98-15.06 ms | 13.73 ms | +0.81 ms, within run-to-run noise |
| handshake to tools/list | 24.87 ms | 21.88-27.27 ms | 24.46 ms | -0.41 ms, within run-to-run noise |
| tools/list response frame | 77,550 bytes | 77,550-77,550 | 90,943 bytes | +13,393 bytes (41 to 44 tools) |

Startup and handshake are effectively unchanged; the catalog carries three more tools and
a 13,393-byte larger discovery frame. The tooling additions in this release did not add a
measurable start-up or handshake cost.

## Reproducibility

Two back-to-back passes of the final script (C at 08:21:35Z, D at 08:22:03Z) agree within
noise:

- Identical in both passes: every byte count, every count row, both comparison frame sizes.
- Startup 13.73 vs 13.95 ms (1.6%); handshake 24.46 vs 24.62 ms (0.6%); browser_click
  133.81 vs 133.89 ms (0.1%); browser_run 20.74 vs 21.72 ms (4.5%); desktop_record
  955.69 vs 961.78 ms (0.6%); launcher 74.53 vs 76.41 ms (2.5%).
- Largest relative moves are on the smallest timings: browser_snapshot 6.84 vs 9.15 ms
  (25.2%), warm navigate 27.73 vs 33.40 ms (17.0%), cold navigate 388.97 vs 439.10 ms
  (11.4%). Their distributions overlap.
- Earlier passes A and B, taken while other sessions were building, had startup medians
  12.21 vs 27.87 ms and comparison medians 39.17 vs 12.32 ms: load moved between their
  phases. No cargo process was running for C and D. These are scheduling artifacts on a
  shared box, not a release difference; C and D are the reported pair.

Commands used, all exit 0: cargo build --locked --release; node benchmarks/perf/run.mjs
(A, B, C, D); node docs-check.mjs --strict.

## Not measured

- Multi-monitor behaviour (one monitor was present; monitor 0 only).
- DPI scaling other than 100% (all reported windows were dpi 96, scale 1).
- Non-elevated desktop targets: all 16 windows observed in this session reported
  elevated:true. The desktop numbers above are from an elevated target.
- 4K capture: the screenshot target was capped at max_px 1280, recording at 960.
- Long recordings: only a 1000 ms recording at 8 fps was measured.
- Network install: npx fetching hand-mcp from the registry (see the note above).
