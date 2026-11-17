// Argument parsing for bench.mjs. No dependency, no hidden flags.

const VALUE_FLAGS = new Set([
  "product",
  "scenario",
  "command",
  "arg",
  "cwd",
  "warmup",
  "measured",
  "timeout-ms",
  "browser-exe",
  "desktop-target",
  "desktop-title-contains",
  "out",
]);

const BOOLEAN_FLAGS = new Set(["help", "list", "json", "test-fixture", "no-display-probe"]);

export function parseArgs(argv) {
  const options = { args: [] };
  const errors = [];
  for (let index = 0; index < argv.length; index += 1) {
    const raw = argv[index];
    if (!raw.startsWith("--")) {
      errors.push(`unexpected positional argument: ${raw}`);
      continue;
    }
    const body = raw.slice(2);
    const equals = body.indexOf("=");
    const flag = equals === -1 ? body : body.slice(0, equals);
    const inlineValue = equals === -1 ? undefined : body.slice(equals + 1);
    if (BOOLEAN_FLAGS.has(flag)) {
      if (inlineValue !== undefined) errors.push(`--${flag} does not take a value`);
      options[toCamel(flag)] = true;
      continue;
    }
    if (!VALUE_FLAGS.has(flag)) {
      errors.push(`unknown flag: --${flag}`);
      continue;
    }
    const value = inlineValue !== undefined ? inlineValue : argv[++index];
    if (value === undefined) {
      errors.push(`--${flag} requires a value`);
      continue;
    }
    if (flag === "arg") {
      options.args.push(value);
    } else {
      options[toCamel(flag)] = value;
    }
  }
  for (const key of ["warmup", "measured", "timeout-ms"]) {
    const camel = toCamel(key);
    if (options[camel] === undefined) continue;
    const parsed = Number(options[camel]);
    if (!Number.isInteger(parsed) || parsed < 0) {
      errors.push(`--${key} must be a non-negative integer`);
    } else {
      options[camel] = parsed;
    }
  }
  return { options, errors };
}

function toCamel(flag) {
  return flag.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
}
