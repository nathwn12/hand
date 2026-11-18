// A persistent Win32 input probe for the benchmark driver.
//
// A pinned baseline (Puppet) reports raw `cursorUnchanged`/`foregroundUnchanged`
// flags but not the attribution hand now carries. On an active desktop those raw
// flags are false whenever the owner moves the mouse during the call, which
// says nothing about the product. This probe samples GetCursorPos,
// GetForegroundWindow and the GetLastInputInfo tick immediately before and
// after the measured call, from outside the product, so the driver can accept a
// raw delta only when real input explains it.
//
// It is one PowerShell process per run, in a read-loop, so a sample is a pipe
// round trip rather than a process start. `close()` always reaps it.

import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

const SAMPLE_TIMEOUT_MS = 5_000;
const CLOSE_TIMEOUT_MS = 5_000;

function parseSample(line) {
  let parsed;
  try {
    parsed = JSON.parse(line);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object") return null;
  return {
    foreground: Number.isInteger(parsed.foreground) ? parsed.foreground : null,
    cursor:
      parsed.cursorReadable === true && Number.isInteger(parsed.cursorX) && Number.isInteger(parsed.cursorY)
        ? [parsed.cursorX, parsed.cursorY]
        : null,
    inputTick: Number.isInteger(parsed.inputTick) ? parsed.inputTick : null,
    at: Date.now(),
  };
}

export async function startInputProbe(scriptPath) {
  const child = spawn(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", scriptPath],
    { windowsHide: true, stdio: ["pipe", "pipe", "pipe"] },
  );
  let stderr = "";
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => {
    stderr = (stderr + chunk).slice(-4000);
  });

  const lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
  const queue = [];
  let pending = null;
  let exited = false;
  let spawnError = null;

  child.once("error", (error) => {
    spawnError = error;
  });
  const exitedPromise = new Promise((resolveExit) => {
    child.once("exit", () => {
      exited = true;
      if (pending) {
        const waiting = pending;
        pending = null;
        waiting.reject(new Error("input probe exited"));
      }
      resolveExit();
    });
  });

  lines.on("line", (line) => {
    const text = line.trim();
    if (text === "") return;
    if (pending) {
      const waiting = pending;
      pending = null;
      waiting.resolve(text);
    } else {
      queue.push(text);
    }
  });

  function nextLine() {
    if (queue.length) return Promise.resolve(queue.shift());
    if (exited) return Promise.reject(new Error("input probe exited"));
    return new Promise((resolveLine, rejectLine) => {
      const timer = setTimeout(() => {
        if (pending) {
          pending = null;
          rejectLine(new Error("input probe sample timed out"));
        }
      }, SAMPLE_TIMEOUT_MS);
      pending = {
        resolve: (value) => {
          clearTimeout(timer);
          resolveLine(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          rejectLine(error);
        },
      };
    });
  }

  async function close() {
    if (exited) return;
    try {
      child.stdin.write("quit\n");
      child.stdin.end();
    } catch {
      // The pipe may already be gone; the exit wait below still reaps it.
    }
    const timer = setTimeout(() => {
      try {
        child.kill();
      } catch {
        // Already gone.
      }
    }, CLOSE_TIMEOUT_MS);
    try {
      await exitedPromise;
    } finally {
      clearTimeout(timer);
    }
  }

  try {
    const banner = await nextLine();
    if (banner !== "ready") {
      await close();
      return null;
    }
  } catch {
    await close();
    return null;
  }
  if (spawnError) {
    await close();
    return null;
  }

  return {
    async sample() {
      if (exited) return null;
      try {
        child.stdin.write("sample\n");
        return parseSample(await nextLine());
      } catch {
        return null;
      }
    },
    close,
    error() {
      return spawnError ? String(spawnError) : stderr.trim() || null;
    },
  };
}
