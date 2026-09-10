#!/usr/bin/env node

import { spawn, spawnSync } from "node:child_process";
import {
  mkdtemp,
  mkdir,
  realpath,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const executableSuffix = process.platform === "win32" ? ".exe" : "";
const appExecutable = resolve(
  process.env.SIKEMUX_E2E_APP ??
    join(
      root,
      "src-tauri",
      "target",
      "e2e",
      "debug",
      `sikemux${executableSuffix}`,
    ),
);
const cliExecutable = resolve(
  process.env.SIKEMUX_E2E_CLI ??
    join(
      root,
      "src-tauri",
      "target",
      "release",
      `sikemux-editor${executableSuffix}`,
    ),
);

const exerciseHarnessTasks = process.argv.includes("--tasks");

const READY_TIMEOUT_MS = 20_000;
const OPEN_TIMEOUT_MS = 70_000;
const PERSIST_TIMEOUT_MS = 10_000;
const MAX_LOG_BYTES = 64 * 1024;

function fail(message, appLog = "") {
  const suffix = appLog.trim() ? `\n\nDesktop output:\n${appLog.trim()}` : "";
  throw new Error(`desktop E2E smoke failed: ${message}${suffix}`);
}

function run(executable, args, env, timeout, argv0) {
  const result = spawnSync(executable, args, {
    cwd: root,
    env,
    encoding: "utf8",
    timeout,
    windowsHide: true,
    ...(argv0 ? { argv0 } : {}),
  });
  return {
    status: result.status,
    signal: result.signal,
    error: result.error,
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
  };
}

function boundedAppend(current, chunk) {
  const combined = `${current}${String(chunk)}`;
  return combined.length <= MAX_LOG_BYTES
    ? combined
    : combined.slice(combined.length - MAX_LOG_BYTES);
}

function delay(milliseconds) {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));
}

async function waitFor(description, timeout, predicate) {
  const started = Date.now();
  while (Date.now() - started < timeout) {
    if (await predicate()) return;
    await delay(100);
  }
  fail(`${description} did not complete within ${timeout} ms`);
}

async function executableExists(path, label) {
  const details = await stat(path).catch(() => null);
  if (!details?.isFile()) fail(`${label} is missing: ${path}`);
}

async function latestStateWriteTime() {
  const candidates = [
    stateDatabase,
    `${stateDatabase}-wal`,
    `${stateDatabase}-shm`,
  ];
  const details = await Promise.all(
    candidates.map((path) => stat(path).catch(() => null)),
  );
  return details.reduce(
    (latest, candidate) => Math.max(latest, candidate?.mtimeMs ?? 0),
    0,
  );
}

async function stopExactChild(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  child.kill("SIGTERM");
  await Promise.race([
    new Promise((resolveExit) => child.once("exit", resolveExit)),
    delay(5_000),
  ]);
  if (child.exitCode === null && child.signalCode === null) {
    child.kill("SIGKILL");
    await new Promise((resolveExit) => child.once("exit", resolveExit));
  }
}

await executableExists(appExecutable, "debug desktop executable");
await executableExists(cliExecutable, "release editor CLI");

const temporaryRoot = await mkdtemp(join(tmpdir(), "sikemux-desktop-e2e-"));
const isolatedHome = join(temporaryRoot, "home");
const project = join(temporaryRoot, "project");
const source = join(project, "smoke.ts");
const endpoint = join(temporaryRoot, "cli-endpoint.json");
const stateDatabase = join(
  isolatedHome,
  ".config",
  "sikemux",
  "state.dev.sqlite3",
);
await mkdir(isolatedHome, { recursive: true });
await mkdir(project, { recursive: true });
const initialized = run(
  "git",
  ["init", "--quiet", project],
  process.env,
  5_000,
);
if (initialized.error || initialized.status !== 0) {
  fail(
    `could not initialize the isolated Git project: ${initialized.error?.message ?? initialized.stderr}`,
  );
}
await writeFile(
  source,
  "export const first = 1;\nexport const second = 2;\n",
  "utf8",
);

if (exerciseHarnessTasks) {
  await writeFile(
    join(project, "sikemux.json"),
    JSON.stringify({
      version: 1,
      tasks: [
        {
          id: "server",
          label: "Harness server test",
          command: `node -e "console.log('READY'); setInterval(() => {}, 1000)"`,
          cwd: ".",
          env: {},
        },
        {
          id: "fail",
          label: "Harness failure test",
          command: `node -e "console.log('expected failure'); process.exit(7)"`,
          cwd: ".",
          env: {},
        },
      ],
    }),
  );
}

const isolatedEnvironment = {
  ...process.env,
  HOME: isolatedHome,
  USERPROFILE: isolatedHome,
  SIKEMUX_CLI_ENDPOINT: endpoint,
  SIKEMUX_BIN_PATH: cliExecutable,
};
delete isolatedEnvironment.SIKEMUX_APP_EXECUTABLE;

let desktopLog = "";
let desktopSpawnError = "";
const desktop = spawn(appExecutable, [], {
  cwd: project,
  env: isolatedEnvironment,
  stdio: ["ignore", "pipe", "pipe"],
  windowsHide: true,
});
desktop.stdout.on("data", (chunk) => {
  desktopLog = boundedAppend(desktopLog, chunk);
});
desktop.stderr.on("data", (chunk) => {
  desktopLog = boundedAppend(desktopLog, chunk);
});
desktop.on("error", (error) => {
  desktopSpawnError = error.message;
});

try {
  await waitFor("authenticated desktop CLI broker", READY_TIMEOUT_MS, () => {
    if (desktopSpawnError) fail(desktopSpawnError, desktopLog);
    if (desktop.exitCode !== null || desktop.signalCode !== null) {
      fail(
        `desktop exited before becoming ready (${desktop.exitCode ?? desktop.signalCode})`,
        desktopLog,
      );
    }
    const status = run(cliExecutable, ["status"], isolatedEnvironment, 2_000);
    return (
      status.status === 0 && /^Sikemux \S+ is running\s*$/u.test(status.stdout)
    );
  });

  // The native broker starts during Tauri setup, before React has necessarily
  // hydrated. Initial persistence is queued only once the WebView is writable,
  // so this is a durable readiness barrier for the renderer-side bridge.
  await waitFor(
    "WebView boot and initial persistence",
    PERSIST_TIMEOUT_MS,
    async () => {
      return (await latestStateWriteTime()) > 0;
    },
  );
  const stateWriteBeforeOpen = await latestStateWriteTime();

  // This call returns success only after the native broker emits the request,
  // the real WebView listener claims it, application state creates/activates
  // an editor pane at the requested location, and the renderer reports the
  // exact target result back through a second Tauri command.
  const opened = run(
    cliExecutable,
    ["open", "--project", project, `${source}:2:3`],
    isolatedEnvironment,
    OPEN_TIMEOUT_MS,
    // The sidecar executable is named `sikemux-editor`, which deliberately
    // defaults to --wait for $EDITOR callers. Exercise ordinary non-waiting
    // `sikemux open` semantics by setting only argv[0], not by renaming or
    // copying the signed sidecar.
    "sikemux",
  );
  if (opened.error) fail(opened.error.message, desktopLog);
  if (opened.signal)
    fail(`editor CLI was terminated by ${opened.signal}`, desktopLog);
  if (opened.status !== 0) {
    fail(
      `editor CLI returned ${opened.status}: ${opened.stderr || opened.stdout}`,
      desktopLog,
    );
  }

  await waitFor(
    "post-open durable state persistence",
    PERSIST_TIMEOUT_MS,
    async () => {
      return (await latestStateWriteTime()) > stateWriteBeforeOpen;
    },
  );

  const harnessEnv = { ...isolatedEnvironment, SIKEMUX_PROJECT: project };
  const inspect = run(
    cliExecutable,
    ["tool", "workspace.inspect"],
    harnessEnv,
    10_000,
  );
  if (inspect.status !== 0)
    fail(`harness inspect failed: ${inspect.stderr}`, desktopLog);
  const workspace = JSON.parse(inspect.stdout);
  if (
    workspace.project !== (await realpath(project)) ||
    !workspace.windows.length ||
    !workspace.cursor
  )
    fail("harness returned an incomplete workspace", desktopLog);
  const reveal = run(
    cliExecutable,
    [
      "tool",
      "ui.open",
      JSON.stringify({ kind: "file", path: "smoke.ts", line: 2 }),
    ],
    harnessEnv,
    10_000,
  );
  if (reveal.status !== 0)
    fail(`harness file open failed: ${reveal.stderr}`, desktopLog);
  const events = run(
    cliExecutable,
    [
      "tool",
      "events.wait",
      JSON.stringify({ cursor: workspace.cursor, timeoutMs: 0 }),
    ],
    harnessEnv,
    10_000,
  );
  if (
    events.status !== 0 ||
    !JSON.parse(events.stdout).events.some(
      (event) => event.kind === "ui.opened",
    )
  )
    fail("harness UI event was not delivered", desktopLog);

  if (exerciseHarnessTasks) {
    const tool = (method, params = {}) => {
      const result = run(
        cliExecutable,
        ["tool", method, JSON.stringify(params)],
        harnessEnv,
        70_000,
      );
      if (result.status !== 0) fail(`${method}: ${result.stderr}`, desktopLog);
      return JSON.parse(result.stdout);
    };
    console.log(
      "Waiting for fixture project trust in the isolated Sikemux window...",
    );
    const started = tool("task.start", {
      taskId: "server",
      idempotencyKey: "server-first",
    });
    if (started.status !== "running") fail("server did not start");
    const repeated = tool("task.start", {
      taskId: "server",
      idempotencyKey: "server-first",
    });
    if (started.executionId !== repeated.executionId) fail("duplicate launch");
    let output;
    await waitFor("task output", 5000, () => {
      output = tool("task.read", { executionId: started.executionId });
      return output.output.includes("READY");
    });
    const incremental = tool("task.read", {
      executionId: started.executionId,
      cursor: output.cursor,
    });
    if (incremental.output !== "") fail("output was repeated");
    tool("ui.open", {
      kind: "terminal",
      executionId: started.executionId,
      focus: true,
    });
    const eventCursor = tool("workspace.inspect").cursor;
    const liveWait = new Promise((resolveWait, rejectWait) => {
      const child = spawn(
        cliExecutable,
        [
          "tool",
          "events.wait",
          JSON.stringify({
            cursor: eventCursor,
            timeoutMs: 5000,
            executionId: started.executionId,
          }),
        ],
        { env: harnessEnv, cwd: project },
      );
      let output = "";
      child.stdout.on("data", (chunk) => {
        output += chunk;
      });
      child.on("error", rejectWait);
      child.on("exit", (code) => {
        if (code === 0) resolveWait(JSON.parse(output));
        else rejectWait(new Error("event wait failed"));
      });
    });
    const stopped = tool("task.stop", { executionId: started.executionId });
    if (
      !(await liveWait).events.some((event) =>
        ["task.stopping", "task.stopped"].includes(event.kind),
      )
    )
      fail("live event wait missed stop");
    if (stopped.status !== "stopped") fail("task did not stop");
    const retained = tool("task.read", { executionId: started.executionId });
    if (!retained.output.includes("READY")) fail("stopped task lost output");
    const failed = tool("task.start", {
      taskId: "fail",
      idempotencyKey: "fail-first",
    });
    await waitFor(
      "failure status",
      5000,
      () =>
        tool("task.read", { executionId: failed.executionId }).exitCode === 7,
    );
    if (
      tool("task.start", { taskId: "server", idempotencyKey: "server-first" })
        .executionId !== started.executionId
    )
      fail("retry after stop spawned a new process");
    console.log(
      "Harness task E2E passed: trust, launch, deduplication, output cursor, terminal reveal, stop, retained output, exit code",
    );
  }

  const afterOpen = run(cliExecutable, ["status"], isolatedEnvironment, 2_000);
  if (afterOpen.status !== 0) {
    fail(
      `desktop stopped responding after the editor flow: ${afterOpen.stderr}`,
      desktopLog,
    );
  }

  console.log(
    "✓ Desktop E2E smoke passed: process → broker → Tauri event/commands → WebView editor → SQLite",
  );
} finally {
  await stopExactChild(desktop);
  await rm(temporaryRoot, {
    recursive: true,
    force: true,
    maxRetries: 10,
    retryDelay: 200,
  });
}
