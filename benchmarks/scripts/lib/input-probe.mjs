
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
    }
    const timer = setTimeout(() => {
      try {
        child.kill();
      } catch {
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
