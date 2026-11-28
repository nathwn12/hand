#!/usr/bin/env node
// Quarantine guard. An ignored test is reported and tracked, never silently skipped.
// Run: node scripts/quarantine-check.mjs
// Exit 0: clean (stale entries are warnings). Exit 1: any violation.

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const LEDGER = path.join(ROOT, '.gates', 'quarantine.yaml');
const FIELDS = ['test', 'issue', 'added', 'owner', 'rearm_after_passes'];

// Scoped source: the browser engine unit tests and the hand crate's test dir.
// TODO(owner): add root tests/ once its three pre-existing #[ignore] attributes
// have ledger entries; until then including it would fail every empty-ledger run.
const SOURCE_DIRS = ['crates/browser-engine/src', 'crates/hand/tests'];

// --- tiny YAML reader for this one fixed schema -----------------------------

function unquote(value) {
  const v = String(value).trim();
  const quoted = (v.startsWith('"') && v.endsWith('"')) || (v.startsWith("'") && v.endsWith("'"));
  return quoted && v.length > 1 ? v.slice(1, -1) : v;
}

function parseLedger(raw) {
  const errors = [];
  const rows = raw.split(/\r?\n/).map((line, i) => ({
    line: i + 1,
    text: line.replace(/(^|\s)#.*$/, '').trimEnd(),
  })).filter((row) => row.text.trim() !== '');

  const head = rows.findIndex((row) => /^tests\s*:/.test(row.text.trim()));
  if (head < 0) return { entries: [], errors: ['line 1: missing required "tests:" key'] };
  rows.slice(0, head).forEach((row) =>
    errors.push(`line ${row.line}: unknown top-level key "${row.text.trim()}"`));

  const declaration = rows[head].text.trim();
  const rest = rows.slice(head + 1);
  if (/^tests:\s*\[\s*\]\s*$/.test(declaration)) {
    if (rest.length) errors.push(`line ${rest[0].line}: entries found after "tests: []"`);
    return { entries: [], errors };
  }
  if (declaration !== 'tests:') {
    errors.push(`line ${rows[head].line}: unsupported tests declaration "${declaration}"`);
    return { entries: [], errors };
  }

  const entries = [];
  let current = null;
  for (const row of rest) {
    const text = row.text.trim();
    const indent = row.text.match(/^\s*/)[0].length;
    const item = /^-\s+([A-Za-z_]\w*)\s*:\s*(.*)$/.exec(text);
    if (item) {
      if (indent < 2) errors.push(`line ${row.line}: entry must be indented under "tests:"`);
      current = { line: row.line, cells: {} };
      current.cells[item[1]] = { value: unquote(item[2]), line: row.line };
      entries.push(current);
      continue;
    }
    const pair = /^([A-Za-z_]\w*)\s*:\s*(.*)$/.exec(text);
    if (!pair) {
      errors.push(`line ${row.line}: expected "- test: <name>" or "key: value"`);
      continue;
    }
    if (!current) {
      errors.push(`line ${row.line}: field "${pair[1]}" appears before any entry`);
      continue;
    }
    current.cells[pair[1]] = { value: unquote(pair[2]), line: row.line };
  }
  return { entries, errors };
}

// --- validators and entry access -------------------------------------------

function isIsoDate(value) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) return false;
  const date = new Date(`${value}T00:00:00Z`);
  return !Number.isNaN(date.getTime()) && date.toISOString().slice(0, 10) === value;
}

function isUrl(value) {
  try {
    return ['http:', 'https:'].includes(new URL(value).protocol);
  } catch {
    return false;
  }
}

function isPositiveInt(value) {
  return /^\d+$/.test(value) && Number.isSafeInteger(Number(value)) && Number(value) > 0;
}

function cell(entry, field) {
  return entry.cells[field]?.value ?? '';
}

function validateEntries(entries) {
  const errors = [];
  const seen = new Map();
  entries.forEach((entry, i) => {
    const where = `line ${entry.line}: entry ${i}`;
    for (const key of Object.keys(entry.cells)) {
      if (!FIELDS.includes(key)) {
        errors.push(`${where} line ${entry.cells[key].line}: unknown field "${key}"`);
      }
    }
    for (const field of FIELDS) {
      if (!cell(entry, field)) errors.push(`${where}: missing required field "${field}"`);
    }
    if (cell(entry, 'test') && seen.has(cell(entry, 'test'))) {
      errors.push(`${where}: duplicate test "${cell(entry, 'test')}" (first at line ${seen.get(cell(entry, 'test'))})`);
    } else if (cell(entry, 'test')) {
      seen.set(cell(entry, 'test'), entry.line);
    }
    if (cell(entry, 'added') && !isIsoDate(cell(entry, 'added'))) {
      errors.push(`${where}: "added" must be an ISO date YYYY-MM-DD, got "${cell(entry, 'added')}"`);
    }
    if (cell(entry, 'issue') && !isUrl(cell(entry, 'issue'))) {
      errors.push(`${where}: "issue" must be an http(s) URL, got "${cell(entry, 'issue')}"`);
    }
    if (cell(entry, 'rearm_after_passes') && !isPositiveInt(cell(entry, 'rearm_after_passes'))) {
      errors.push(`${where}: "rearm_after_passes" must be a positive integer, got "${cell(entry, 'rearm_after_passes')}"`);
    }
  });
  return errors;
}

// --- source cross-check -----------------------------------------------------

const IGNORE_ATTR = /^\s*#\[ignore(?:\s*(?:=|\]))/;
const FN_DECL = /^\s*(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_]\w*)/;

function walk(dir, files) {
  let entries;
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true });
  } catch (error) {
    if (error.code === 'ENOENT') return;
    throw error;
  }
  entries.sort((a, b) => a.name.localeCompare(b.name));
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full, files);
    else if (entry.isFile()) files.push(full);
  }
}

function findFunction(lines, start) {
  for (let i = start + 1; i < Math.min(lines.length, start + 12); i += 1) {
    const match = FN_DECL.exec(lines[i]);
    if (match) return match[1];
    if (/^\s*#\[/.test(lines[i])) continue;
    if (/^\s*(?:fn|struct|enum|impl|mod)\b/.test(lines[i])) break;
  }
  return null;
}

function collectIgnores() {
  const found = [];
  for (const dir of SOURCE_DIRS) {
    const files = [];
    walk(path.join(ROOT, dir), files);
    for (const file of files) {
      const lines = fs.readFileSync(file, 'utf8').split(/\r?\n/);
      lines.forEach((line, i) => {
        if (!IGNORE_ATTR.test(line)) return;
        const name = findFunction(lines, i);
        const source = path.relative(ROOT, file).split(path.sep).join('/');
        found.push({ id: name ?? `${source}:${i + 1}`, name, source, line: i + 1 });
      });
    }
  }
  return found.sort((a, b) => a.source.localeCompare(b.source) || a.line - b.line);
}

function matches(test, ignore) {
  return test === ignore.id || (ignore.name !== null && test.endsWith(`::${ignore.name}`));
}

// --- reporting --------------------------------------------------------------

function ageInDays(added) {
  const then = Date.parse(`${added}T00:00:00Z`);
  return Math.max(0, Math.floor((Date.now() - then) / 86400000));
}

function printTable(entries) {
  const rows = entries.map((entry) => [
    cell(entry, 'test'),
    cell(entry, 'issue'),
    String(ageInDays(cell(entry, 'added'))),
    `passes >= ${cell(entry, 'rearm_after_passes')} consecutive (flake-rate artifact)`,
  ]);
  const headers = ['test', 'issue', 'age (days)', 'rearm condition'];
  const widths = headers.map((header, col) =>
    Math.max(header.length, ...rows.map((row) => row[col].length)));
  const render = (cells) => cells.map((value, col) => value.padEnd(widths[col])).join(' | ');
  process.stdout.write(`${render(headers)}\n`);
  process.stdout.write(`${widths.map((w) => '-'.repeat(w)).join('-+-')}\n`);
  if (!rows.length) process.stdout.write('(none)\n');
  else rows.forEach((row) => process.stdout.write(`${render(row)}\n`));
}

function finish(violations, warnings) {
  warnings.forEach((message) => process.stdout.write(`warning: ${message}\n`));
  violations.forEach((message) => process.stderr.write(`quarantine-check: ${message}\n`));
  if (violations.length) {
    process.stderr.write(`quarantine-check: ${violations.length} violation(s), exit 1\n`);
    process.exitCode = 1;
  } else {
    process.stdout.write(`quarantine-check: 0 violation(s), exit 0\n`);
  }
}

// --- main -------------------------------------------------------------------

function main() {
  let raw;
  try {
    raw = fs.readFileSync(LEDGER, 'utf8');
  } catch (error) {
    finish([`cannot read ledger .gates/quarantine.yaml: ${error.message}`], []);
    return;
  }
  const { entries, errors } = parseLedger(raw);
  const violations = [...errors, ...validateEntries(entries)];
  if (violations.length) {
    finish(violations, []);
    return;
  }

  const ignores = collectIgnores();
  const used = new Set();
  const warnings = [];
  for (const ignore of ignores) {
    const index = entries.findIndex((entry, i) => !used.has(i) && matches(cell(entry, 'test'), ignore));
    if (index < 0) {
      violations.push(`unledgered #[ignore] at ${ignore.source}:${ignore.line} (test "${ignore.id}")`);
    } else {
      used.add(index);
    }
  }
  entries.forEach((entry, i) => {
    if (!used.has(i)) warnings.push(`stale entry "${cell(entry, 'test')}" (line ${entry.line}): no #[ignore] found in source`);
  });

  printTable(entries);
  process.stdout.write(`quarantine-check: ${entries.length} ${entries.length === 1 ? 'entry' : 'entries'}, ${ignores.length} ignored test(s)\n`);
  finish(violations, warnings);
}

main();
