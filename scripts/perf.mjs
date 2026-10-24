import { execFile } from "node:child_process";
import { createServer as createNetServer } from "node:net";
import { dirname, resolve } from "node:path";
import { parseArgs, promisify } from "node:util";
import { webkit } from "playwright-core";
import { build, preview } from "vite";

/*
 * Times the app in WebKit, the engine it runs on, on a production build of the
 * showcase, and checks each scenario against what a person would notice as
 * slow. The machine is often busy, so each limit passes once any run meets
 * it. `node scripts/perf.mjs terminal` runs one scenario.
 */

const root = resolve(import.meta.dirname, "..");
const configFile = resolve(root, "showcase/vite.perf.config.ts");
const { values: options, positionals } = parseArgs({
  allowPositionals: true,
  options: { runs: { type: "string", default: "3" } },
});

const FRAME_120HZ_MS = 1000 / 120;

const usageLine = (usage) =>
  usage
    ? `content CPU ${usage.contentCpuPercent}%, RSS ${usage.contentRssMb} MB, GPU process ${usage.gpuCpuPercent}%, UI process ${usage.uiCpuPercent}%`
    : "no process usage";
const chatPhase = (phase) =>
  [
    `frame p50/p90/p99/longest ${phase.p50Ms}/${phase.p90Ms}/${phase.p99Ms}/${phase.longestMs} ms`,
    `${phase.dropped} of ${phase.frames} dropped`,
    `render p50/p90/p99/longest ${phase.render.p50Ms}/${phase.render.p90Ms}/${phase.render.p99Ms}/${phase.render.longestMs} ms`,
    usageLine(phase.usage),
  ].join(", ");
/**
 * Each limit is in milliseconds. A "smooth" one is a frame target for a 120Hz
 * display and grows with a slower one. The rest hold on any display: a frame
 * over 50 ms is a visible hitch, and so is a typed key that shows later than
 * that. Hitches are counted at the 99th percentile, so one stray frame from
 * another process on the machine does not fail a run.
 */
const SCENARIOS = {
  chat: {
    options: { rows: 2000, page: 200, tokens: 4000 },
    describe: (result) => [
      `open ${result.rows} rows: settled in ${result.open.settledMs} ms, ${chatPhase(result.open)}`,
      `scroll to the top: ${chatPhase(result.scroll)}`,
      `scroll up as ${result.paging.pages} pages arrive: ${chatPhase(result.paging)}`,
      `  place kept for ${result.paging.placeKept} of ${result.paging.pages} pages${result.paging.jumpsPx.length ? `, jumped ${result.paging.jumpsPx.join(", ")} px` : ""}`,
      `stream ${result.stream.chars} characters in ${result.stream.flushes} flushes: ${chatPhase(result.stream)}, ${result.stream.fromBottomPx} px from the bottom after`,
    ],
    limits: [
      ["scroll p90", (result) => result.scroll.p90Ms, 14, "smooth"],
      ["paging scroll p90", (result) => result.paging.p90Ms, 14, "smooth"],
      ["stream p90", (result) => result.stream.p90Ms, 17, "smooth"],
      [
        "frame p99",
        (result) =>
          Math.max(
            result.scroll.p99Ms,
            result.paging.p99Ms,
            result.stream.p99Ms,
          ),
        50,
      ],
    ],
  },
  git: {
    options: { dirtyFiles: 1500 },
    describe: (result) => [
      `${result.dirtyFiles} dirty files`,
      `open: over budget ${result.open.overBudgetMs} ms, longest ${result.open.longestMs} ms`,
    ],
    limits: [
      ["scroll p50", (result) => result.scroll.p50Ms, 9, "smooth"],
      ["scroll p90", (result) => result.scroll.p90Ms, 14, "smooth"],
      ["fling p90", (result) => result.fling.p90Ms, 20, "smooth"],
      [
        "switch longest (median)",
        (result) => result.switch.longestMs,
        20,
        "smooth",
      ],
      ["switch worst", (result) => result.switch.worstMs, 50, "smooth"],
    ],
  },
  terminal: {
    options: { seconds: 5, keys: 25 },
    describe: (result) => [
      `flood: ${result.flood.megabytesPerSecond} MB/s, frame p50 ${result.flood.p50Ms} ms`,
      `typing: ${result.typing.keys} keys, keystroke to paint p50 ${result.typing.p50Ms} ms, worst ${result.typing.longestMs} ms`,
      `scrollback: longest frame ${Math.max(result.scroll.longestMs, result.fling.longestMs)} ms`,
    ],
    limits: [
      ["flood frame p90", (result) => result.flood.p90Ms, 34],
      ["flood frame p99", (result) => result.flood.p99Ms, 50],
      ["keystroke to paint p90", (result) => result.typing.p90Ms, 50],
      ["scrollback scroll p90", (result) => result.scroll.p90Ms, 17, "smooth"],
      ["scrollback fling p90", (result) => result.fling.p90Ms, 17, "smooth"],
      [
        "scrollback frame p99",
        (result) => Math.max(result.scroll.p99Ms, result.fling.p99Ms),
        50,
      ],
    ],
  },
  theme: {
    options: { steps: 12 },
    describe: (result) =>
      Object.entries(result).map(
        ([window, r]) =>
          `${window}: shown p50 ${r.p50Ms} ms, longest ${r.longestMs} ms`,
      ),
    limits: [
      [
        "theme shown p50",
        (result) => Math.max(...Object.values(result).map((r) => r.p50Ms)),
        100,
      ],
    ],
  },
  startup: {
    options: {},
    describe: (result) => [
      `before the first pane: frame p50 ${result.beforePane.p50Ms} ms, longest ${result.beforePane.longestMs} ms`,
    ],
    limits: [
      ["first pane drawn", (result) => result.firstPaneMs, 1000],
      // Nothing moves yet as the app settles, so a hitch shows only as a click or key that waits, and under 100 ms that feels immediate.
      ["longest frame after it", (result) => result.afterPane.longestMs, 100],
    ],
  },
};

const chosen = positionals.length > 0 ? positionals : Object.keys(SCENARIOS);
for (const name of chosen) {
  if (!SCENARIOS[name]) {
    console.error(
      `no scenario "${name}"; there are ${Object.keys(SCENARIOS).join(", ")}`,
    );
    process.exit(2);
  }
}

/** `pnpm showcase:perf` keeps 5791, so this takes whichever port is free. */
const freePort = () =>
  new Promise((settle, fail) => {
    const probe = createNetServer().listen(0, "localhost", () => {
      const { port } = probe.address();
      probe.close(() => settle(port));
    });
    probe.on("error", fail);
  });

process.env.NODE_ENV = "production";
await build({ root, configFile, mode: "production", logLevel: "warn" });
const port = await freePort();
const server = await preview({
  root,
  configFile,
  mode: "production",
  logLevel: "silent",
  preview: { port, strictPort: true },
});
const url = `http://localhost:${port}/showcase/perf.html`;

const run = promisify(execFile);
const webkitDir = dirname(webkit.executablePath());

/** WebKit's processes from this browser's build, which other agents' runs may share. */
async function webkitProcesses() {
  const { stdout } = await run("ps", ["-axo", "pid=,rss=,command="]);
  return stdout.split("\n").flatMap((line) => {
    const [, pid, rss, command] = line.match(/^\s*(\d+)\s+(\d+)\s+(.*)$/) ?? [];
    return command?.startsWith(webkitDir)
      ? [{ pid: Number(pid), rss: Number(rss), command }]
      : [];
  });
}

const elsewhere = new Set((await webkitProcesses()).map(({ pid }) => pid));

/** The page's content process is the one that came with it, or failing that the biggest of this browser's. */
async function pageProcesses(before) {
  const ours = (await webkitProcesses()).filter(
    ({ pid }) => !elsewhere.has(pid),
  );
  const biggest = (list) => list.sort((a, b) => b.rss - a.rss)[0]?.pid;
  const content = ours.filter(({ command }) => command.includes("WebContent"));
  return {
    content:
      biggest(content.filter(({ pid }) => !before.has(pid))) ??
      biggest(content),
    gpu: biggest(ours.filter(({ command }) => command.includes("WebKit.GPU"))),
    ui: biggest(
      ours.filter(({ command }) => command.includes("/MacOS/Playwright")),
    ),
  };
}

const cpuSeconds = (time) =>
  time.split(":").reduce((sum, part) => sum * 60 + Number(part), 0);

async function sample(pids) {
  const { stdout } = await run("ps", [
    "-o",
    "pid=,time=,rss=",
    "-p",
    pids.join(","),
  ]);
  const read = new Map();
  for (const line of stdout.trim().split("\n")) {
    const [pid, time, rss] = line.trim().split(/\s+/);
    read.set(Number(pid), { cpu: cpuSeconds(time), rssMb: Number(rss) / 1024 });
  }
  return { at: performance.now(), read };
}

/**
 * Answers the page's `perfUsage(true)` and `perfUsage(false)` around a phase
 * with the share of a core each process kept busy over it, from their CPU
 * time, and the most memory the content process held, sampled every 250 ms.
 */
function usageWatcher(processes) {
  const pids = [processes.content, processes.gpu, processes.ui].filter(Boolean);
  let from = null;
  let peak = 0;
  let timer = null;
  const rssOf = (read) => read.get(processes.content)?.rssMb ?? 0;
  return async (watching) => {
    if (!processes.content) return null;
    if (watching) {
      from = await sample(pids);
      peak = rssOf(from.read);
      timer = setInterval(async () => {
        peak = Math.max(peak, rssOf((await sample(pids)).read));
      }, 250);
      return null;
    }
    clearInterval(timer);
    const to = await sample(pids);
    const percent = (pid) => {
      const [start, end] = [from.read.get(pid), to.read.get(pid)];
      return start && end
        ? Math.round(((end.cpu - start.cpu) / ((to.at - from.at) / 1000)) * 100)
        : null;
    };
    return {
      contentCpuPercent: percent(processes.content),
      contentRssMb: Math.round(Math.max(peak, rssOf(to.read))),
      gpuCpuPercent: percent(processes.gpu),
      uiCpuPercent: percent(processes.ui),
    };
  };
}

const browser = await webkit.launch();
const context = await browser.newContext({
  viewport: { width: 1600, height: 1000 },
  colorScheme: "dark",
});
// The showcase reads its wallpaper and pane image from this machine's home folder, so every machine times the app without them.
await context.route(/\/__showcase\/(wallpaper|preview)/, (route) =>
  route.abort(),
);

async function open() {
  const before = new Set((await webkitProcesses()).map(({ pid }) => pid));
  const page = await context.newPage();
  page.on("pageerror", (error) => {
    if (!error.message.startsWith("ResizeObserver loop"))
      console.error(`page error: ${error.message}`);
  });
  await page.exposeFunction("perfPress", (key) => page.keyboard.press(key));
  await page.goto(url);
  await page.waitForFunction(() => window.showcase && window.perf, null, {
    timeout: 60_000,
  });
  await page.exposeFunction(
    "perfUsage",
    usageWatcher(await pageProcesses(before)),
  );
  return page;
}

function verdict(scenario, result) {
  const slower = Math.max(1, result.frameMs / FRAME_120HZ_MS || 1);
  return scenario.limits.map(([name, read, limit, kind]) => {
    const scaled = kind === "smooth" ? Math.round(limit * slower) : limit;
    const value = read(result);
    return { name, value, limit: scaled, ok: value <= scaled };
  });
}

function print(name, run, scenario, result, checks) {
  const display = result.frameMs
    ? `, ${Math.round(1000 / result.frameMs)}Hz`
    : "";
  console.log(`\n${name} run ${run}${display}`);
  for (const line of scenario.describe(result)) console.log(`        ${line}`);
  for (const { name: check, value, limit, ok } of checks) {
    console.log(
      `  ${ok ? "pass" : "FAIL"}  ${check.padEnd(26)} ${String(value).padStart(5)} ms  (≤ ${limit})`,
    );
  }
}

const failed = [];
try {
  // The first load fills the cache, as the app's own files are always on disk.
  await (await open()).close();
  for (const name of chosen) {
    const scenario = SCENARIOS[name];
    const met = new Set();
    for (
      let run = 1;
      run <= Number(options.runs) && met.size < scenario.limits.length;
      run++
    ) {
      const page = await open();
      const result = await page.evaluate(
        ([scenarioName, scenarioOptions]) =>
          window.perf.run(scenarioName, scenarioOptions),
        [name, scenario.options],
      );
      await page.close();
      const checks = verdict(scenario, result);
      print(name, run, scenario, result, checks);
      for (const check of checks) if (check.ok) met.add(check.name);
    }
    if (met.size < scenario.limits.length) failed.push(name);
  }
} finally {
  await browser.close();
  await server.close();
}

console.log(
  failed.length === 0
    ? `\n${chosen.join(", ")}: every scenario meets its limits`
    : `\n${failed.join(", ")}: some limit was missed in every run`,
);
process.exit(failed.length === 0 ? 0 : 1);
