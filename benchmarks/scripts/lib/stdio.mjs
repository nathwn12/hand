// One MCP stdio session for benchmarking.
//
// This is intentionally stricter than a convenience client:
// - one request in flight at a time (a second request throws);
// - no retries, no reconnection, no batch support;
// - a reply is matched by JSON-RPC id; anything else is a protocol violation;
// - every request resolves with {ok, elapsedMs, ...}; a transport failure or
//   JSON-RPC error is data, not an exception, because failures must be counted;
// - latency is measured from the stdin write to the complete response frame.
//
// Both pinned baseline servers speak newline-delimited JSON-RPC over stdio with
// logging on stderr. The driver never assumes Content-Length framing.

import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { createInterface } from "node:readline";

const DEFAULT_TIMEOUT_MS = 30_000;
const STDERR_TAIL_CHARS = 4096;
const STDERR_CAP_CHARS = 65_536;

export class BenchClient {
  constructor({
    command,
    args = [],
    cwd,
    env = {},
    timeoutMs = DEFAULT_TIMEOUT_MS,
    clientName = "hand-mcp-bench",
  }) {
    if (!command) throw new Error("command is required");
    this.command = command;
    this.args = args;
    this.timeoutMs = timeoutMs;
    this.clientName = clientName;
    this.pending = new Map();
    this.protocolViolations = [];
    this.unsolicitedFrames = 0;
    this.notifications = 0;
    this.seq = 0;
    this.inFlight = 0;
    this.maxInFlight = 0;
    this.requestCount = 0;
    this.stderrBytes = 0;
    this.stderrText = "";
    this.stderrTail = "";
    this.stderrTruncated = false;
    this.exited = null;
    this.spawnedAtNs = process.hrtime.bigint();
    this.spawnedAtIso = new Date().toISOString();

    this.child = spawn(command, args, {
      cwd,
      windowsHide: true,
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, ...env },
    });
    this.child.stdout.setEncoding("utf8");
    this.child.stderr.setEncoding("utf8");

    this.child.stderr.on("data", (chunk) => this.#onStderr(chunk));
    this.reader = createInterface({ input: this.child.stdout, crlfDelay: Infinity });
    this.reader.on("line", (line) => this.#onLine(line));

    this.exitPromise = new Promise((resolve) => {
      this.child.on("exit", (code, signal) => {
        this.exited = { code, signal, atMs: this.msSinceSpawn() };
        this.#failPending(`server exited (code=${code} signal=${signal})`);
        resolve(this.exited);
      });
      this.child.on("error", (error) => {
        this.exited = { code: null, signal: null, error: String(error), atMs: this.msSinceSpawn() };
        this.#failPending(`server failed to start: ${error}`);
        resolve(this.exited);
      });
    });
  }

  msSinceSpawn() {
    return Number(process.hrtime.bigint() - this.spawnedAtNs) / 1e6;
  }

  #onStderr(chunk) {
    this.stderrBytes += Buffer.byteLength(chunk, "utf8");
    if (this.stderrText.length < STDERR_CAP_CHARS) {
      this.stderrText += chunk.slice(0, STDERR_CAP_CHARS - this.stderrText.length);
    } else {
      this.stderrTruncated = true;
    }
    this.stderrTail = (this.stderrTail + chunk).slice(-STDERR_TAIL_CHARS);
  }

  #onLine(line) {
    if (!line.trim()) return;
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      this.protocolViolations.push({ reason: "unparseable frame", line: line.slice(0, 400) });
      this.unsolicitedFrames += 1;
      return;
    }
    const id = message.id;
    if (id === undefined) {
      if (typeof message.method === "string") {
        this.notifications += 1;
        return;
      }
      this.protocolViolations.push({ reason: "frame had neither id nor method", line: line.slice(0, 400) });
      this.unsolicitedFrames += 1;
      return;
    }
    if (!this.pending.has(id)) {
      this.protocolViolations.push({ reason: "frame did not match a pending request", line: line.slice(0, 400) });
      this.unsolicitedFrames += 1;
      return;
    }
    const waiter = this.pending.get(id);
    this.pending.delete(id);
    clearTimeout(waiter.timer);
    const elapsedMs = Number(process.hrtime.bigint() - waiter.startedNs) / 1e6;
    this.inFlight -= 1;
    this.requestCount += 1;
    if (message.error) {
      waiter.resolve({
        ok: false,
        elapsedMs,
        error: `JSON-RPC error ${message.error.code}: ${message.error.message}`,
        rpcError: { code: message.error.code, message: message.error.message },
        raw: message,
        dispatched: true,
      });
      return;
    }
    waiter.resolve({ ok: true, elapsedMs, result: message.result, raw: message, dispatched: true });
  }

  #failPending(reason) {
    for (const [id, waiter] of this.pending) {
      clearTimeout(waiter.timer);
      const elapsedMs = Number(process.hrtime.bigint() - waiter.startedNs) / 1e6;
      this.pending.delete(id);
      this.inFlight -= 1;
      waiter.resolve({ ok: false, elapsedMs, error: reason, dispatched: true });
    }
  }

  writeFrame(payload) {
    const line = JSON.stringify(payload);
    this.child.stdin.write(`${line}\n`);
    return line;
  }

  notify(method, params) {
    const payload = { jsonrpc: "2.0", method };
    if (params !== undefined) payload.params = params;
    return this.writeFrame(payload);
  }

  async request(method, params, { timeoutMs = this.timeoutMs } = {}) {
    if (this.inFlight > 0) {
      throw new Error(`concurrent request refused: ${method} while ${this.inFlight} still in flight`);
    }
    if (this.exited) {
      return {
        ok: false,
        elapsedMs: null,
        error: `server already exited (code=${this.exited.code} signal=${this.exited.signal})`,
        dispatched: false,
      };
    }
    const id = (this.seq += 1);
    const payload = { jsonrpc: "2.0", id, method };
    if (params !== undefined) payload.params = params;
    const startedNs = process.hrtime.bigint();
    this.inFlight = 1;
    this.maxInFlight = 1;

    const promise = new Promise((resolve) => {
      const timer = setTimeout(() => {
        if (!this.pending.has(id)) return;
        this.pending.delete(id);
        this.inFlight -= 1;
        resolve({
          ok: false,
          elapsedMs: Number(process.hrtime.bigint() - startedNs) / 1e6,
          error: `${method} timed out after ${timeoutMs} ms`,
          timedOut: true,
          dispatched: true,
        });
        // The late reply is recorded as a protocol violation by #onLine.
      }, timeoutMs);
      this.pending.set(id, {
        resolve: (value) => resolve(value),
        startedNs,
        timer,
      });
    });

    try {
      const line = JSON.stringify(payload);
      this.child.stdin.write(`${line}\n`);
    } catch (error) {
      const waiter = this.pending.get(id);
      if (waiter) {
        clearTimeout(waiter.timer);
        this.pending.delete(id);
        this.inFlight -= 1;
      }
      return {
        ok: false,
        elapsedMs: Number(process.hrtime.bigint() - startedNs) / 1e6,
        error: `stdin write failed: ${error}`,
        dispatched: true,
      };
    }
    return promise;
  }

  async initialize() {
    const response = await this.request("initialize", {
      protocolVersion: "2025-11-25",
      capabilities: {},
      clientInfo: { name: this.clientName, version: "1.0.0" },
    });
    if (response.ok) {
      this.notify("notifications/initialized", {});
    }
    return response;
  }

  async listTools() {
    const response = await this.request("tools/list", {});
    if (!response.ok) return response;
    const tools = response.result?.tools;
    if (!Array.isArray(tools)) {
      return { ...response, ok: false, error: "tools/list returned no tools[] array" };
    }
    return {
      ...response,
      tools,
      names: tools.map((tool) => tool.name),
      catalogSha256: `sha256:${createHash("sha256").update(JSON.stringify(tools)).digest("hex")}`,
    };
  }

  stderrReceipt() {
    return {
      bytes: this.stderrBytes,
      truncated: this.stderrTruncated,
      tail: this.stderrTail,
    };
  }

  async close({ graceMs = 3_000 } = {}) {
    if (this.exited) return this.exited;
    try {
      this.child.stdin.end();
    } catch {
      // stdin may already be closed
    }
    const timer = new Promise((resolve) => setTimeout(() => resolve({ code: null, signal: "timeout" }), graceMs));
    const outcome = await Promise.race([this.exitPromise, timer]);
    if (!this.exited) {
      this.child.kill();
      await Promise.race([this.exitPromise, new Promise((resolve) => setTimeout(resolve, graceMs))]);
    }
    return outcome;
  }
}
