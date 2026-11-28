#!/usr/bin/env node
// scripts/flake-rate.mjs
// Turn repeated browser-engine runs into a base rate, not a pass/fail gate.
// Usage: node scripts/flake-rate.mjs [--dir DIR] [--out FILE] [--log FILE ...]
// Reads flake-run-<n>.log by default, prints a markdown summary, appends it to
// $GITHUB_STEP_SUMMARY when set, and writes flake-rate.json. Exit is always 0.

import fs from 'node:fs';
import path from 'node:path';

const ANSI = /\u001B\[[0-9;]*[A-Za-z]/g;
const TEST_LINE = /^test\s+(.+?)\s+\.\.\.\s+(ok|FAILED|ignored|bench)(?:\s|$)/;
const SECTION = /^---- (.+?) stdout ----\s*$/;
const THREAD_PANIC = /^thread '([^']+)' panicked at\b/;
const RESULT = /^test result: (ok|FAILED)\./;
const DEFAULT_OUT = 'flake-rate.json';
const COMMAND = 'cargo test --locked -p browser-engine -- --test-threads=1';
const BT = '\x60';

const HELP = [
  'Usage: node scripts/flake-rate.mjs [options]',
  '  --dir DIR    directory holding flake-run-<n>.log (default: cwd)',
  '  --out FILE   JSON output path (default: DIR/flake-rate.json)',
  '  --log FILE   parse this log instead of discovery (repeatable)',
  '  -h, --help   show this help',
  'Exit code is always 0: this is a measurement, not a gate.'
].join('\n');

function clean(line) {
  return line.replace(ANSI, '').replace(/\r$/, '').trimEnd();
}

function rate(failures, seen) {
  return seen > 0 ? Number((failures / seen).toFixed(6)) : 0;
}

function pct(value) {
  return (value * 100).toFixed(1) + '%';
}

function cell(text) {
  return String(text).replace(/\|/g, '\\|');
}

function parseArgs(argv) {
  const opts = { dir: process.cwd(), out: null, logs: [], help: false, warnings: [] };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '-h' || arg === '--help') {
      opts.help = true;
    } else if (arg === '--dir' || arg === '--out' || arg === '--log') {
      const value = argv[i + 1];
      i += 1;
      if (!value) {
        opts.warnings.push(arg + ' needs a value; ignored');
      } else if (arg === '--dir') {
        opts.dir = path.resolve(value);
      } else if (arg === '--out') {
        opts.out = path.resolve(value);
      } else {
        opts.logs.push(path.resolve(value));
      }
    } else if (arg.startsWith('--dir=')) {
      opts.dir = path.resolve(arg.slice(6));
    } else if (arg.startsWith('--out=')) {
      opts.out = path.resolve(arg.slice(6));
    } else if (arg.startsWith('--log=')) {
      opts.logs.push(path.resolve(arg.slice(6)));
    } else {
      opts.warnings.push('unknown argument ignored: ' + arg);
    }
  }
  if (!opts.out) opts.out = path.resolve(opts.dir, DEFAULT_OUT);
  return opts;
}

function discover(dir) {
  return fs.readdirSync(dir)
    .filter(function (name) { return /^flake-run-\d+\.log$/.test(name); })
    .sort(function (a, b) {
      const na = Number(/flake-run-(\d+)\.log/.exec(a)[1]);
      const nb = Number(/flake-run-(\d+)\.log/.exec(b)[1]);
      return na - nb;
    })
    .map(function (name) { return path.join(dir, name); });
}

function parseLog(file) {
  const lines = fs.readFileSync(file, 'utf8').split(/\r?\n/).map(clean);
  const seen = new Set();
  const failed = new Set();
  const signatures = new Map();
  const results = [];
  let section = null;

  function addSignature(name, signature) {
    if (!name || !seen.has(name)) return;
    if (!signatures.has(name)) signatures.set(name, new Set());
    signatures.get(name).add(signature);
  }

  for (const line of lines) {
    const header = SECTION.exec(line);
    if (header) {
      section = header[1];
      continue;
    }
    if (line === 'failures:') {
      section = null;
      continue;
    }
    const testLine = TEST_LINE.exec(line);
    if (testLine) {
      seen.add(testLine[1]);
      if (testLine[2] === 'FAILED') failed.add(testLine[1]);
      continue;
    }
    if (/\bpanicked at\b/.test(line)) {
      const thread = THREAD_PANIC.exec(line);
      const target = section || (thread && seen.has(thread[1]) ? thread[1] : null);
      addSignature(target, line.trim());
      continue;
    }
    const result = RESULT.exec(line);
    if (result) results.push(result[1]);
  }
  return {
    log: path.basename(file),
    tests_seen: seen.size,
    failures: failed.size,
    result: results.length ? results[results.length - 1] : 'unknown',
    failed_tests: Array.from(failed).sort(),
    seen: seen,
    failed: failed,
    signatures: signatures
  };
}

function aggregate(runs) {
  const byName = new Map();
  let totalFailures = 0;
  let totalSeen = 0;
  for (const run of runs) {
    totalSeen += run.seen.size;
    totalFailures += run.failures;
    for (const name of run.seen) {
      if (!byName.has(name)) {
        byName.set(name, { name: name, runs_seen: 0, failures: 0, signatures: new Set() });
      }
      const entry = byName.get(name);
      entry.runs_seen += 1;
      if (run.failed.has(name)) {
        entry.failures += 1;
        const sigs = run.signatures.get(name);
        if (sigs && sigs.size > 0) {
          for (const sig of sigs) entry.signatures.add(sig);
        } else {
          entry.signatures.add('(no panic line found in log)');
        }
      }
    }
  }
  const tests = Array.from(byName.values())
    .sort(function (a, b) { return a.name.localeCompare(b.name); })
    .map(function (entry) {
      return {
        name: entry.name,
        runs_seen: entry.runs_seen,
        failures: entry.failures,
        rate: rate(entry.failures, entry.runs_seen),
        signatures: Array.from(entry.signatures).sort()
      };
    });
  return { tests: tests, totalFailures: totalFailures, totalSeen: totalSeen, overallRate: rate(totalFailures, totalSeen) };
}

function markdown(report, warnings) {
  const out = [
    '# browser-engine flake rate',
    '',
    'Measured: ' + report.runs.length + ' run(s), ' + report.totals.test_runs_seen + ' test-runs seen, ' + report.totals.failures + ' failure(s), overall ' + pct(report.totals.overall_rate) + '.',
    '',
    'Command per run: ' + BT + COMMAND + BT + ' (no retries, no assertion changes).',
    '',
    '## Per run',
    '',
    '| Run | Log | Harness result | Failed tests |',
    '| --- | --- | --- | --- |'
  ];
  report.runs.forEach(function (run, index) {
    const names = run.failed_tests.length
      ? run.failed_tests.map(function (name) { return BT + cell(name) + BT; }).join(', ')
      : 'none';
    out.push('| ' + (index + 1) + ' | ' + BT + cell(run.log) + BT + ' | ' + cell(run.result) + ' | ' + names + ' |');
  });
  out.push('', '## Per test', '');
  out.push('| Test | Runs seen | Failures | Rate | Failure signatures (panic lines) |');
  out.push('| --- | ---: | ---: | ---: | --- |');
  for (const test of report.tests) {
    const sigs = test.signatures.length
      ? test.signatures.map(function (sig) { return BT + cell(sig) + BT; }).join('<br>')
      : '-';
    out.push('| ' + BT + cell(test.name) + BT + ' | ' + test.runs_seen + ' | ' + test.failures + ' | ' + pct(test.rate) + ' | ' + sigs + ' |');
  }
  if (report.tests.length === 0) out.push('| _no test lines found_ | 0 | 0 | 0.0% | - |');
  out.push('');
  out.push('Rate = failures / runs seen for that test. Signatures are the union of distinct panic lines seen on failing runs.');
  if (warnings.length > 0) {
    out.push('', 'Warnings:', '');
    for (const warning of warnings) out.push('- ' + cell(warning));
  }
  out.push('');
  return out.join('\n');
}

function writeSummary(markdownText) {
  process.stdout.write(markdownText);
  const target = process.env.GITHUB_STEP_SUMMARY;
  if (target) fs.appendFileSync(target, markdownText + '\n', 'utf8');
}

function reportOf(runs) {
  const agg = aggregate(runs);
  return {
    generated_at: new Date().toISOString(),
    command: COMMAND,
    runs: runs.map(function (run) {
      return { log: run.log, tests_seen: run.tests_seen, failures: run.failures, result: run.result, failed_tests: run.failed_tests };
    }),
    totals: { runs: runs.length, test_runs_seen: agg.totalSeen, failures: agg.totalFailures, overall_rate: agg.overallRate },
    tests: agg.tests
  };
}

function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.help) {
    process.stdout.write(HELP);
    return;
  }
  const warnings = opts.warnings.slice();
  let logs;
  if (opts.logs.length > 0) {
    logs = opts.logs;
  } else {
    try {
      logs = discover(opts.dir);
    } catch (error) {
      warnings.push('cannot list ' + opts.dir + ': ' + error.message);
      logs = [];
    }
  }
  if (logs.length === 0) warnings.push('no flake-run-<n>.log files found in ' + opts.dir);
  const runs = [];
  for (const log of logs) {
    try {
      runs.push(parseLog(log));
    } catch (error) {
      warnings.push('could not parse ' + path.basename(log) + ': ' + error.message);
    }
  }
  const report = reportOf(runs);
  fs.mkdirSync(path.dirname(opts.out), { recursive: true });
  fs.writeFileSync(opts.out, JSON.stringify(report, null, 2) + '\n', 'utf8');
  writeSummary(markdown(report, warnings));
}

try {
  main();
} catch (error) {
  const message = 'flake-rate: measurement failed, emitting an empty report: ' + (error && error.message ? error.message : String(error));
  const empty = {
    generated_at: new Date().toISOString(),
    command: COMMAND,
    runs: [],
    totals: { runs: 0, test_runs_seen: 0, failures: 0, overall_rate: 0 },
    tests: [],
    warnings: [message]
  };
  try {
    fs.writeFileSync(path.resolve(process.cwd(), DEFAULT_OUT), JSON.stringify(empty, null, 2) + '\n', 'utf8');
  } catch (ignored) {}
  try {
    writeSummary('# browser-engine flake rate\n\n- ' + message + '\n');
  } catch (ignored) {}
}
