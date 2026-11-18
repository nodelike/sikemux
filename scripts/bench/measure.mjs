#!/usr/bin/env node
// Measures one running desktop app at idle: memory (phys_footprint, what Activity Monitor
// shows) and CPU, for the app's own processes and, apart, for the agent CLIs it started.
//
//   node scripts/bench/measure.mjs --app "/Applications/Orca.app" [--idle 300] [--out run.json]
//
// The app's processes are every process whose program lives inside the bundle, and every
// process those started. Agent CLIs are the `claude` processes among them and what they
// started, unless that program is itself inside the bundle.
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir, userInfo } from "node:os";
import { join } from "node:path";

const options = { idle: 300, interval: 30 };
for (let i = 2; i < process.argv.length; i += 2) options[process.argv[i].replace(/^--/, "")] = process.argv[i + 1];
if (!options.app) {
  console.error("usage: measure.mjs --app /Applications/Name.app [--idle seconds] [--interval seconds] [--out file.json]");
  process.exit(2);
}
const bundle = options.app.replace(/\/$/, "");
const idle = Number(options.idle);
const interval = Number(options.interval);

const run = (command, args) => execFileSync(command, args, { encoding: "utf8", maxBuffer: 64 << 20 });

function plist(key) {
  try {
    return run("/usr/libexec/PlistBuddy", ["-c", `Print ${key}`, `${bundle}/Contents/Info.plist`]).trim();
  } catch {
    return null;
  }
}

const seconds = (time) => time.split(":").reduce((total, part) => total * 60 + Number(part), 0);

function processes() {
  const table = new Map();
  for (const line of run("ps", ["-axww", "-o", "pid=,ppid=,time=,rss=,comm="]).split("\n")) {
    const match = line.match(/^\s*(\d+)\s+(\d+)\s+(\S+)\s+(\d+)\s+(.*)$/);
    if (match) {
      table.set(Number(match[1]), {
        pid: Number(match[1]),
        ppid: Number(match[2]),
        cpu: seconds(match[3]),
        rss: Number(match[4]) * 1024,
        program: match[5],
      });
    }
  }
  for (const line of run("ps", ["-axww", "-o", "pid=,args="]).split("\n")) {
    const match = line.match(/^\s*(\d+)\s+(.*)$/);
    if (match && table.has(Number(match[1]))) table.get(Number(match[1])).args = match[2];
  }
  return table;
}

const insideBundle = (entry) => entry.program.startsWith(`${bundle}/`);
const isAgentCli = (entry) =>
  /(^|\/)claude$/.test(entry.program) ||
  /\/claude\/versions\//.test(entry.program) ||
  /(claude-code|claude-agent-sdk)[^ ]*\/cli\.js/.test(entry.args ?? "");

function classify(table) {
  const children = new Map();
  for (const entry of table.values()) {
    if (!children.has(entry.ppid)) children.set(entry.ppid, []);
    children.get(entry.ppid).push(entry);
  }
  const own = new Map();
  const agents = new Map();
  const others = new Map();
  const walk = (entry, underAgent) => {
    if (own.has(entry.pid) || agents.has(entry.pid) || others.has(entry.pid)) return;
    const agent = !insideBundle(entry) && (underAgent || isAgentCli(entry));
    const into = insideBundle(entry) ? own : agent ? agents : others;
    into.set(entry.pid, entry);
    for (const child of children.get(entry.pid) ?? []) walk(child, agent);
  };
  for (const entry of table.values()) if (insideBundle(entry)) walk(entry, false);
  return { own: [...own.values()], agents: [...agents.values()], others: [...others.values()] };
}

const scratch = mkdtempSync(join(tmpdir(), "bench-footprint-"));
function footprint(entries) {
  if (entries.length === 0) return { total: 0, perProcess: new Map() };
  const file = join(scratch, "footprint.json");
  const args = ["--noCategories", "-f", "bytes", "-j", file];
  for (const entry of entries) args.push("-p", String(entry.pid));
  spawnSync("footprint", args, { stdio: "ignore" });
  const report = JSON.parse(readFileSync(file, "utf8"));
  return {
    total: report["total footprint"],
    perProcess: new Map(report.processes.map((p) => [p.pid, p.footprint])),
  };
}

function sample(table) {
  const { own, agents, others } = classify(table);
  return {
    own: footprint(own),
    others: footprint(others),
    app: footprint([...own, ...others]),
    agents: footprint(agents),
    both: footprint([...own, ...others, ...agents]),
    ownEntries: own,
    otherEntries: others,
    appEntries: [...own, ...others],
    agentEntries: agents,
  };
}

const mb = (bytes) => `${(bytes / 2 ** 20).toFixed(0)} MB`;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const cpuOf = (entries) => entries.reduce((sum, entry) => sum + entry.cpu, 0);

const before = processes();
const first = classify(before);
if (first.own.length === 0) {
  console.error(`nothing is running from ${bundle}`);
  process.exit(1);
}
const started = Date.now();
const samples = [];
while (true) {
  const table = processes();
  const taken = sample(table);
  samples.push({
    at: Math.round((Date.now() - started) / 1000),
    agentCount: taken.agentEntries.filter(isAgentCli).length,
    app: taken.app.total,
    agents: taken.agents.total,
    both: taken.both.total,
  });
  const latest = samples.at(-1);
  console.error(`${latest.at}s  app ${mb(latest.app)}  agents ${mb(latest.agents)} (${latest.agentCount} running)`);
  if (Date.now() - started >= idle * 1000) break;
  await sleep(Math.min(interval * 1000, idle * 1000 - (Date.now() - started)));
}
const elapsed = (Date.now() - started) / 1000;
const after = processes();
const last = sample(after);

function cpuPercent(entriesAfter) {
  let used = 0;
  for (const entry of entriesAfter) used += entry.cpu - (before.get(entry.pid)?.cpu ?? 0);
  return (used / elapsed) * 100;
}

const describe = (entries, prints) =>
  entries
    .map((entry) => ({
      pid: entry.pid,
      program: entry.program.replace(`${bundle}/`, ""),
      footprint: prints.perProcess.get(entry.pid) ?? null,
      rss: entry.rss,
        cpuSeconds: Number((entry.cpu - (before.get(entry.pid)?.cpu ?? 0)).toFixed(2)),
    }))
    .sort((a, b) => (b.footprint ?? 0) - (a.footprint ?? 0));

const sysctl = (key) => run("sysctl", ["-n", key]).trim();
const result = {
  machine: {
    model: sysctl("hw.model"),
    chip: sysctl("machdep.cpu.brand_string"),
    memory: Number(sysctl("hw.memsize")),
    macos: run("sw_vers", ["-productVersion"]).trim(),
    user: userInfo().username,
  },
  app: bundle,
  name: plist("CFBundleName"),
  version: plist("CFBundleShortVersionString"),
  build: plist("CFBundleVersion"),
  measuredAt: new Date().toISOString(),
  idleSeconds: Number(elapsed.toFixed(0)),
  agentCount: last.agentEntries.filter(isAgentCli).length,
  memory: {
    own: last.own.total,
    others: last.others.total,
    app: last.app.total,
    agents: last.agents.total,
    both: last.both.total,
    appPeak: Math.max(...samples.map((s) => s.app)),
    bothPeak: Math.max(...samples.map((s) => s.both)),
    appRss: last.appEntries.reduce((sum, e) => sum + e.rss, 0),
    agentsRss: last.agentEntries.reduce((sum, e) => sum + e.rss, 0),
  },
  cpu: {
    ownPercent: Number(cpuPercent(last.ownEntries).toFixed(2)),
    othersPercent: Number(cpuPercent(last.otherEntries).toFixed(2)),
    appPercent: Number(cpuPercent(last.appEntries).toFixed(2)),
    agentsPercent: Number(cpuPercent(last.agentEntries).toFixed(2)),
    appSecondsBeforeIdle: Number(cpuOf([...first.own, ...first.others]).toFixed(1)),
  },
  samples,
  processes: { own: describe(last.ownEntries, last.own), others: describe(last.otherEntries, last.others), agents: describe(last.agentEntries, last.agents) },
};
rmSync(scratch, { recursive: true, force: true });
const text = JSON.stringify(result, null, 2);
if (options.out) writeFileSync(options.out, `${text}\n`);
else console.log(text);
console.error(
  `${result.name} ${result.version}: ${result.agentCount} agents · app ${mb(result.memory.app)} · with agents ${mb(result.memory.both)} · CPU app ${result.cpu.appPercent}% agents ${result.cpu.agentsPercent}%`,
);
