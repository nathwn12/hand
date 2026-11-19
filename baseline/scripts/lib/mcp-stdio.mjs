
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

const DEFAULT_TIMEOUT_MS = 30_000;

export class StdioClient {
  constructor({ command, args = [], cwd, env = {}, timeoutMs = DEFAULT_TIMEOUT_MS }) {
    this.command = command;
    this.args = args;
    this.timeoutMs = timeoutMs;
    this.pending = new Map();
    this.transcript = [];
    this.stderr = "";
    this.unsolicited = [];
    this.seq = 0;
    this.exited = null;
    this.child = spawn(command, args, {
      cwd,
      windowsHide: true,
      stdio: ["pipe", "pipe", "pipe"],
      env: childEnv(env),
    });
    this.child.stdout.setEncoding("utf8");
    this.child.stderr.setEncoding("utf8");
    this.child.stderr.on("data", (chunk) => {
      this.stderr += chunk;
    });
    const rl = createInterface({ input: this.child.stdout, crlfDelay: Infinity });
    rl.on("line", (line) => this.#onLine(line));
    this.exitPromise = new Promise((resolve) => {
      this.child.on("exit", (code, signal) => {
        this.exited = { code, signal };
        for (const [, waiter] of this.pending) waiter.reject(new Error(`server exited (code=${code} signal=${signal}) before replying`));
        this.pending.clear();
        resolve(this.exited);
      });
      this.child.on("error", (error) => {
        this.exited = { code: null, signal: null, error: String(error) };
        resolve(this.exited);
      });
    });
  }

  #onLine(line) {
    if (!line.trim()) return;
    this.transcript.push({ dir: "recv", at: new Date().toISOString(), line });
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      this.unsolicited.push({ reason: "unparseable frame", line });
      return;
    }
    const id = message.id;
    if (id !== undefined && this.pending.has(id)) {
      const waiter = this.pending.get(id);
      this.pending.delete(id);
      waiter.resolve(message);
      return;
    }
    this.unsolicited.push({ reason: "no matching request", message });
  }

  #send(payload) {
    const line = JSON.stringify(payload);
    this.transcript.push({ dir: "send", at: new Date().toISOString(), line });
    this.child.stdin.write(`${line}\n`);
    return line;
  }

  async request(method, params, { timeoutMs = this.timeoutMs } = {}) {
    if (this.exited) throw new Error(`cannot send ${method}: server exited`);
    const id = (this.seq += 1);
    const payload = { jsonrpc: "2.0", id, method };
    if (params !== undefined) payload.params = params;
    const started = process.hrtime.bigint();
    const promise = new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method} timed out after ${timeoutMs} ms`));
      }, timeoutMs);
      this.pending.set(id, {
        resolve: (message) => {
          clearTimeout(timer);
          resolve(message);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      });
    });
    this.#send(payload);
    const response = await promise;
    const elapsedMs = Number(process.hrtime.bigint() - started) / 1e6;
    if (response.error) {
      throw new Error(`${method} returned JSON-RPC error ${response.error.code}: ${response.error.message}`);
    }
    return { id, result: response.result, elapsedMs: round(elapsedMs), raw: response };
  }

  notify(method, params) {
    const payload = { jsonrpc: "2.0", method };
    if (params !== undefined) payload.params = params;
    this.#send(payload);
  }

  async close({ graceMs = 5_000 } = {}) {
    if (this.child.exitCode !== null) return this.exited;
    try {
      this.child.stdin.end();
    } catch {
    }
    const timer = new Promise((resolve) => setTimeout(() => resolve({ code: null, signal: "timeout" }), graceMs));
    const outcome = await Promise.race([this.exitPromise, timer]);
    if (this.child.exitCode === null) {
      this.child.kill();
      await this.exitPromise.catch(() => {});
    }
    return outcome;
  }
}

function childEnv(extra) {
  const env = { ...process.env, ...extra };
  delete env.GLADIUS_EXPECTED_VERSION;
  return env;
}

function round(ms) {
  return Math.round(ms * 1000) / 1000;
}
