#!/usr/bin/env node
// One-command 30-task browser sequence from fixtures/tasks.json.
//
//   node fixtures/run-tasks.mjs [--only <id>] [--json] [--repeat <n>]
//
// The hand binary is copied to a temp directory before launch: the main tree is
// rebuilt by another lane and a running Windows executable cannot be replaced.
// Tool discovery is live, so a task whose tool or capability is absent becomes
// `pending` with the missing name instead of a false pass or a silent skip.

import { createHash } from 'node:crypto';
import { copyFile, mkdir, mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { StdioClient } from '../baseline/scripts/lib/mcp-stdio.mjs';
import { startFixtureServer } from './serve.mjs';
import { startInteractionFixture } from './interaction/serve.mjs';

const root = fileURLToPath(new URL('.', import.meta.url));
const repoRoot = resolve(root, '..');
const manifestFile = join(root, 'tasks.json');
const CALL_TIMEOUT_MS = 180_000;

function usage() {
  return [
    'usage: node fixtures/run-tasks.mjs [--only <id>] [--json] [--repeat <n>] [--bin <path>]',
    '',
    '  --only <id>   run one task id',
    '  --json        emit one JSON result object instead of task lines',
    '  --repeat <n>  run the whole sequence n times (default 1)',
    '  --bin <path>  hand executable to copy (default target/debug/hand.exe)',
  ].join('\n');
}

function parseArgs(argv) {
  const out = { only: undefined, json: false, repeat: 1, bin: undefined, help: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--help' || arg === '-h') { out.help = true; continue; }
    if (arg === '--json') { out.json = true; continue; }
    if (arg === '--only' || arg === '--repeat' || arg === '--bin') {
      const value = argv[i + 1];
      if (!value || value.startsWith('--')) throw new Error(`${arg} requires a value`);
      if (arg === '--only') out.only = value;
      else if (arg === '--bin') out.bin = value;
      else {
        if (!/^\d+$/.test(value) || Number(value) < 1) throw new Error('--repeat must be a positive integer');
        out.repeat = Number(value);
      }
      i += 1;
      continue;
    }
    throw new Error(`unknown argument: ${arg}`);
  }
  return out;
}

function deepEqual(a, b) {
  if (Object.is(a, b)) return true;
  if (typeof a !== typeof b) return false;
  if (a === null || b === null || typeof a !== 'object') return false;
  return JSON.stringify(a) === JSON.stringify(b);
}

function subsetOf(actual, expected) {
  return Object.entries(expected).every(([key, value]) => deepEqual(actual?.[key], value));
}

function getPath(record, dotted) {
  let value = record;
  for (const part of String(dotted).split('.')) {
    if (value === undefined || value === null) return undefined;
    value = value[part];
  }
  return value;
}

function describeValue(value) {
  const text = JSON.stringify(value);
  return text === undefined ? String(value) : text;
}

function checkAssertion(assertion, results) {
  const actual = getPath(results[assertion.call], assertion.path);
  const expected = assertion.value;
  let ok = false;
  switch (assertion.op) {
    case 'eq': ok = deepEqual(actual, expected); break;
    case 'ne': ok = !deepEqual(actual, expected); break;
    case 'truthy': ok = Boolean(actual); break;
    case 'falsy': ok = !actual; break;
    case 'exists': ok = actual !== undefined && actual !== null; break;
    case 'codeIn': ok = Array.isArray(expected) && expected.includes(actual); break;
    case 'gte': ok = Number(actual) >= Number(expected); break;
    case 'lte': ok = Number(actual) <= Number(expected); break;
    case 'match': ok = new RegExp(String(expected)).test(String(actual)); break;
    case 'notContains': ok = !containsValue(actual, expected); break;
    case 'contains': ok = containsValue(actual, expected); break;
    default: throw new Error(`unknown assertion op: ${assertion.op}`);
  }
  return {
    ok,
    detail: ok ? '' : `call ${assertion.call} ${assertion.path}: ${assertion.op} ${describeValue(expected)}, got ${describeValue(actual)}`,
  };
}

function containsValue(actual, expected) {
  if (typeof actual === 'string') return actual.includes(String(expected));
  if (Array.isArray(actual)) return actual.some((item) => deepEqual(item, expected) || (expected && typeof expected === 'object' && subsetOf(item, expected)));
  if (actual && typeof actual === 'object') return Object.prototype.hasOwnProperty.call(actual, String(expected));
  return false;
}

function evaluateExpect(expect, results) {
  if (!expect) return { ok: true, detail: '' };
  const groups = [];
  if (expect.all) groups.push({ mode: 'all', assertions: expect.all });
  if (expect.any) groups.push({ mode: 'any', assertions: expect.any });
  const failures = [];
  for (const group of groups) {
    const checks = group.assertions.map((a) => checkAssertion(a, results));
    const ok = group.mode === 'all' ? checks.every((c) => c.ok) : checks.some((c) => c.ok);
    if (!ok) failures.push(...checks.filter((c) => !c.ok).map((c) => c.detail));
  }
  return { ok: failures.length === 0, detail: failures.join('; ') };
}

function messageOf(result) {
  const text = Array.isArray(result?.content)
    ? result.content.find((item) => item?.type === 'text' && typeof item.text === 'string')?.text
    : undefined;
  return typeof text === 'string' ? text : '';
}

function payloadOf(result) {
  if (result?.structuredContent && typeof result.structuredContent === 'object') return result.structuredContent;
  const text = messageOf(result);
  if (text.trim()) {
    try { return JSON.parse(text); } catch { return { text }; }
  }
  return {};
}

// A typed not_implemented is an absent capability, not a broken one. The
// tool's own prose names the limit, so it becomes the pending reason. Other
// typed errors stay failures: only this exact code promotes a task to pending.
function notImplementedReason(record) {
  if (!record.isError || record.payload?.code !== 'not_implemented') return null;
  const keys = ['kind', 'action', 'field', 'name'];
  const key = keys.find((name) => record.payload[name] !== undefined);
  const name = key ? `${record.tool}(${key}=${record.payload[key]})` : record.tool;
  return `waits for ${name}: ${record.message || record.payload.code}`;
}

function resolveTokens(value, tokens) {
  if (typeof value === 'string') {
    let text = value;
    for (const [token, replacement] of Object.entries(tokens)) text = text.split(token).join(String(replacement));
    return text;
  }
  if (Array.isArray(value)) return value.map((item) => resolveTokens(item, tokens));
  if (value && typeof value === 'object') {
    const out = {};
    for (const [key, item] of Object.entries(value)) out[key] = resolveTokens(item, tokens);
    return out;
  }
  return value;
}

function applyBindings(bindings, results, tokens) {
  for (const rule of bindings ?? []) {
    const source = results[rule.call];
    if (!source) continue;
    const list = getPath(source, rule.path);
    if (!Array.isArray(list)) continue;
    const match = list.find((item) => {
      if (rule.where && !subsetOf(item, rule.where)) return false;
      if (rule.whereContains && !Object.entries(rule.whereContains).every(([key, needle]) => String(item?.[key] ?? '').includes(String(needle)))) return false;
      return true;
    });
    if (match && match[rule.pick] !== undefined) {
      tokens[`{{${rule.name}}}`] = `${rule.prefix ?? ''}${match[rule.pick]}`;
    }
  }
}

async function copyBinary(source, destination) {
  let lastError;
  for (let attempt = 0; attempt < 5; attempt += 1) {
    try {
      await copyFile(source, destination);
      return;
    } catch (error) {
      lastError = error;
      await new Promise((r) => setTimeout(r, 200));
    }
  }
  throw new Error(`could not copy ${source}: ${lastError?.message ?? lastError}`);
}

function capabilityGap(requirement, tools) {
  const schema = tools.get(requirement.tool);
  if (!schema) return requirement.tool;
  if (!requirement.field) return null;
  const properties = schema.properties ?? {};
  if (!(requirement.field in properties)) {
    if (schema.additionalProperties === true) return null;
    return `${requirement.tool}(${requirement.field})`;
  }
  if (requirement.value !== undefined) {
    const property = properties[requirement.field];
    const values = property?.enum ?? property?.items?.enum;
    if (Array.isArray(values) && !values.includes(requirement.value)) {
      return `${requirement.tool}(${requirement.field}=${requirement.value})`;
    }
  }
  return null;
}

function missingNames(task, tools) {
  const gaps = [];
  for (const requirement of task.requires ?? []) {
    const gap = capabilityGap(requirement, tools);
    if (gap && !gaps.includes(gap)) gaps.push(gap);
  }
  for (const call of [...(task.calls ?? []), ...(task.cleanup ?? [])]) {
    if (!tools.has(call.tool) && !gaps.includes(call.tool)) gaps.push(call.tool);
  }
  return gaps;
}

async function runOnce({ manifest, client, tools, tokens, runNumber }) {
  const results = [];
  for (const task of manifest.tasks) {
    const gaps = missingNames(task, tools);
    let status;
    let reason = '';
    const callRecords = [];
    if (gaps.length > 0) {
      status = 'pending';
      reason = `waits for ${gaps.join(', ')}`;
    } else {
      const localTokens = { ...tokens };
      for (let index = 0; index < task.calls.length; index += 1) {
        const call = task.calls[index];
        const args = resolveTokens(call.args ?? {}, localTokens);
        let record;
        try {
          const response = await client.request('tools/call', { name: call.tool, arguments: args }, { timeoutMs: CALL_TIMEOUT_MS });
          const result = response.result ?? {};
          const isError = result.isError === true;
          const payload = payloadOf(result);
          record = { tool: call.tool, args, isError, error: isError ? payload.code ?? payload.message ?? '' : '', payload, message: messageOf(result), elapsedMs: response.elapsedMs };
        } catch (error) {
          record = { tool: call.tool, args, isError: true, error: error.message, payload: {}, message: '', elapsedMs: 0 };
        }
        callRecords.push(record);
        applyBindings(task.bind, callRecords, localTokens);
      }
      const verdict = evaluateExpect(task.expect, callRecords);
      status = verdict.ok ? 'pass' : 'fail';
      reason = verdict.detail;
      const absent = callRecords.map(notImplementedReason).find(Boolean);
      if (absent) {
        status = 'pending';
        reason = absent;
      }
      const cleanupErrors = [];
      for (const call of task.cleanup ?? []) {
        try {
          const response = await client.request('tools/call', { name: call.tool, arguments: resolveTokens(call.args ?? {}, localTokens) }, { timeoutMs: CALL_TIMEOUT_MS });
          if (response.result?.isError === true) cleanupErrors.push(`${call.tool}: ${describeValue(payloadOf(response.result).code ?? 'isError')}`);
        } catch (error) {
          cleanupErrors.push(`${call.tool}: ${error.message}`);
        }
      }
      if (cleanupErrors.length > 0) reason = `${reason} cleanup: ${cleanupErrors.join('; ')}`.trim();
    }
    const result = { id: task.id, title: task.title, status, reason, calls: callRecords };
    results.push(result);
    if (globalThis.__handTasksJson !== true) {
      const label = status === 'pass' ? 'PASS' : status === 'pending' ? 'PEND' : 'FAIL';
      const suffix = reason ? `  ${reason}` : '';
      console.log(`[${String(results.length).padStart(2)}/${manifest.tasks.length}] ${label}  ${task.id}${suffix}`);
    }
  }
  const counts = { pass: 0, pending: 0, fail: 0 };
  for (const result of results) counts[result.status] += 1;
  return { run: runNumber, counts, tasks: results };
}

async function main() {
  let options;
  try { options = parseArgs(process.argv.slice(2)); }
  catch (error) { console.error(`${error.message}\n${usage()}`); process.exitCode = 2; return; }
  if (options.help) { console.log(usage()); return; }
  globalThis.__handTasksJson = options.json;

  const manifest = JSON.parse(await readFile(manifestFile, 'utf8'));
  const selected = options.only ? manifest.tasks.filter((task) => task.id === options.only) : manifest.tasks;
  if (selected.length === 0) {
    console.error(`unknown task id: ${options.only}`);
    process.exitCode = 2;
    return;
  }
  const runManifest = { ...manifest, tasks: selected };

  const sourceBin = options.bin ? resolve(options.bin) : join(repoRoot, 'target', 'debug', 'hand.exe');
  const tempRoot = await mkdtemp(join(tmpdir(), 'hand-tasks-'));
  const binDir = join(tempRoot, 'bin');
  await mkdir(binDir, { recursive: true });
  const binCopy = join(binDir, 'hand.exe');
  let client;
  let fixture;
  let interaction;
  try {
    await copyBinary(sourceBin, binCopy);
    const sha = createHash('sha256').update(await readFile(binCopy)).digest('hex').slice(0, 12);

    interaction = await startInteractionFixture();
    fixture = await startFixtureServer({
      port: 0,
      substitutions: { __CROSS_ORIGIN__: () => interaction.origins.b },
    });

    client = new StdioClient({
      command: binCopy,
      args: [],
      cwd: binDir,
      env: {
        GLADIUS_CDP_URL: '',
        GLADIUS_BACKEND_CAPS: 'render',
        HAND_TEST_HOOKS: '0',
        GLADIUS_TOOL_BUDGET_MS: String(CALL_TIMEOUT_MS),
        HAND_TOOL_BUDGET_MS: String(CALL_TIMEOUT_MS),
      },
      timeoutMs: CALL_TIMEOUT_MS,
    });

    await client.request('initialize', {
      protocolVersion: '2025-11-25',
      capabilities: {},
      clientInfo: { name: 'hand-task-runner', version: '1.0.0' },
    });
    client.notify('notifications/initialized');
    const listed = await client.request('tools/list', {});
    const tools = new Map((listed.result?.tools ?? []).map((tool) => [tool.name, tool.inputSchema ?? {}]));

    const tokens = {
      '{{origin}}': fixture.origin,
      '{{originB}}': interaction.origins.b,
      '{{tempDir}}': tempRoot,
      '{{uploadFile}}': join(root, 'tasks', 'upload.txt'),
    };

    const runs = [];
    let fatal = false;
    for (let number = 1; number <= options.repeat; number += 1) {
      if (!options.json) console.log(`run ${number}/${options.repeat}  binary ${basename(sourceBin)} sha256:${sha}`);
      try {
        runs.push(await runOnce({ manifest: runManifest, client, tools, tokens, runNumber: number }));
      } catch (error) {
        fatal = true;
        console.error(`run ${number} aborted: ${error.stack ?? error}`);
        break;
      }
      const last = runs[runs.length - 1].counts;
      if (!options.json) console.log(`summary run ${number}/${options.repeat}: pass=${last.pass} pending=${last.pending} fail=${last.fail}`);
    }

    const total = { pass: 0, pending: 0, fail: 0 };
    for (const run of runs) {
      total.pass += run.counts.pass;
      total.pending += run.counts.pending;
      total.fail += run.counts.fail;
    }
    const exitCode = fatal || total.fail > 0 ? 1 : 0;
    if (options.json) {
      console.log(JSON.stringify({ manifestVersion: manifest.manifestVersion, binary: { path: sourceBin, sha256: sha }, runs, total, exitCode }));
    } else {
      console.log(`summary all runs: pass=${total.pass} pending=${total.pending} fail=${total.fail} exit=${exitCode}`);
    }
    process.exitCode = exitCode;
  } finally {
    if (client) await client.close().catch(() => {});
    if (fixture) await new Promise((r) => fixture.server.close(r));
    if (interaction) await interaction.close().catch(() => {});
    await rm(tempRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 }).catch(() => {});
  }
}

function basename(path) {
  return String(path).split(/[\\/]/).pop();
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  main().catch((error) => {
    console.error(`FATAL ${error.stack ?? error}`);
    process.exitCode = 1;
  });
}
