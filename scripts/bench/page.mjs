#!/usr/bin/env node
// Turns the JSON files run.mjs wrote into one HTML page: method, machine, versions, numbers.
//
//   node scripts/bench/page.mjs [results dir] [out.html]
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const dir = process.argv[2] ?? join(here, "results");
const out = process.argv[3] ?? join(dir, "index.html");
const order = ["sikemux", "t3code", "orca", "conductor"];
if (!existsSync(dir)) {
  console.error(`no results folder at ${dir}; run run.mjs first`);
  process.exit(1);
}
const runs = order.filter((name) => existsSync(join(dir, `${name}.json`))).map((name) => ({ key: name, ...JSON.parse(readFileSync(join(dir, `${name}.json`), "utf8")) }));
if (runs.length === 0) {
  console.error(`no results in ${dir}: ${readdirSync(dir).join(", ") || "it is empty"}`);
  process.exit(1);
}

const escape = (value) => String(value ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const gb = (bytes) => (bytes / 2 ** 30).toFixed(2);
const mb = (bytes) => `${Math.round(bytes / 2 ** 20).toLocaleString("en")} MB`;
const machine = runs[0].machine;
const widest = Math.max(...runs.map((r) => r.memory.both));

const bars = runs
  .map((r) => {
    const app = (r.memory.app / widest) * 100;
    const agents = (r.memory.agents / widest) * 100;
    return `<div class="bar-row"><div class="bar-name">${escape(r.name)}</div><div class="bar"><span class="app" style="width:${app}%"></span><span class="agents" style="width:${agents}%"></span></div><div class="bar-value">${gb(r.memory.app)} GB <small>+ ${gb(r.memory.agents)} GB agents</small></div></div>`;
  })
  .join("");

const rows = runs
  .map(
    (r) => `<tr>
  <td><b>${escape(r.name)}</b><br><small>${escape(r.version)}${r.build && r.build !== r.version ? ` (${escape(r.build)})` : ""}</small></td>
  <td>${r.sessionsAnswered}/${r.sessionsOpened}<br><small>${r.agentCount} claude running</small></td>
  <td class="n">${gb(r.memory.app)} GB<br><small>peak ${gb(r.memory.appPeak)}</small></td>
  <td class="n">${gb(r.memory.agents)} GB</td>
  <td class="n">${gb(r.memory.both)} GB<br><small>peak ${gb(r.memory.bothPeak)}</small></td>
  <td class="n">${r.cpu.appPercent.toFixed(1)}%</td>
  <td class="n">${r.cpu.agentsPercent.toFixed(1)}%</td>
</tr>`,
  )
  .join("");

const breakdown = runs
  .map((r) => {
    const list = (title, entries) =>
      entries.length === 0
        ? ""
        : `<h4>${title}</h4><table class="procs">${entries
            .slice(0, 12)
            .map((p) => `<tr><td>${escape(p.program)}</td><td class="n">${mb(p.footprint ?? 0)}</td><td class="n">${p.cpuSeconds}s CPU</td></tr>`)
            .join("")}${entries.length > 12 ? `<tr><td colspan="3"><small>and ${entries.length - 12} more</small></td></tr>` : ""}</table>`;
    return `<details><summary>${escape(r.name)}: ${r.processes.own.length + r.processes.others.length} app processes, ${r.processes.agents.length} agent processes</summary>
${list("App, inside the bundle", r.processes.own)}${list("App, started by it from outside the bundle", r.processes.others)}${list("Agent CLIs and what they started", r.processes.agents)}
<p><small>claude ${escape(r.claude?.version)} · measured ${escape(r.measuredAt)} over ${r.idleSeconds} s</small></p></details>`;
  })
  .join("");

const html = `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Memory at idle with 20 agent sessions</title>
<style>
:root { color-scheme: dark; --bg: #0e0e10; --surface: #17171a; --line: #26262b; --ink: #ececf1; --muted: #9a9aa3; --app: #8b7cf6; --agents: #3a3a44; }
body { margin: 0; background: var(--bg); color: var(--ink); font: 14px/1.55 Figtree, -apple-system, sans-serif; }
main { max-width: 920px; margin: 0 auto; padding: 48px 24px 80px; }
h1 { font-size: 26px; margin: 0 0 6px; } h2 { font-size: 16px; margin: 40px 0 12px; } h4 { margin: 16px 0 6px; color: var(--muted); font-weight: 500; }
p, li { color: var(--muted); } b { color: var(--ink); } small { color: var(--muted); }
.bar-row { display: grid; grid-template-columns: 150px 1fr 220px; gap: 12px; align-items: center; margin: 10px 0; }
.bar { display: flex; height: 18px; background: var(--surface); border-radius: 6px; overflow: hidden; }
.bar .app { background: var(--app); } .bar .agents { background: var(--agents); }
.bar-value { font-variant-numeric: tabular-nums; }
table { width: 100%; border-collapse: collapse; } th, td { text-align: left; padding: 10px 8px; border-bottom: 1px solid var(--line); vertical-align: top; }
th { color: var(--muted); font-weight: 500; font-size: 12px; } .n { text-align: right; font-variant-numeric: tabular-nums; }
.procs td { padding: 4px 8px; font-size: 12px; } details { background: var(--surface); border-radius: 8px; padding: 10px 14px; margin: 8px 0; } summary { cursor: pointer; }
.key { display: inline-block; width: 10px; height: 10px; border-radius: 3px; margin: 0 6px 0 14px; vertical-align: middle; }
</style></head><body><main>
<h1>Memory at idle with ${runs[0].sessionsOpened} Claude Code sessions open</h1>
<p>${escape(machine.model)}, ${escape(machine.chip)}, ${Math.round(machine.memory / 2 ** 30)} GB, macOS ${escape(machine.macos)}. Each app started fresh, opened ${runs[0].sessionsOpened} sessions, each answered once, then sat idle for ${Math.round(runs[0].idleSeconds / 60)} minutes.</p>
<h2>Memory after the idle period<span class="key" style="background:var(--app)"></span><small>app</small><span class="key" style="background:var(--agents)"></span><small>agent CLIs</small></h2>
${bars}
<h2>Numbers</h2>
<table><tr><th>App</th><th>Sessions answered</th><th class="n">App memory</th><th class="n">Agent CLIs</th><th class="n">Together</th><th class="n">App CPU at idle</th><th class="n">Agents CPU at idle</th></tr>${rows}</table>
<h2>Method</h2>
<ul>
<li><b>Memory</b> is physical footprint (what Activity Monitor's Memory column shows), from <code>footprint</code>, with memory shared between an app's processes counted once.</li>
<li><b>App</b> is every process whose program is inside the app bundle (main, helpers, renderers, servers) plus anything those start that is not an agent: shells, git, language servers.</li>
<li><b>Agent CLIs</b> are the <code>claude</code> processes the app started and everything under them. They are reported apart, and together with the app.</li>
<li><b>CPU</b> is CPU time used over the idle period, as a percentage of one core.</li>
<li>No tokens are spent: every session is the real Claude Code CLI, pointed at a local stand-in for Anthropic's Messages API that answers each turn with one word.</li>
<li>Runs happen one app at a time in a macOS user kept for the benchmark, with nobody else logged in, each app starting from no saved state.</li>
<li>The scripts are in the Sikemux repository under <code>scripts/bench/</code>: <code>node scripts/bench/run.mjs all</code>.</li>
</ul>
<h2>What differs between the apps</h2>
<ul>
<li>Sikemux and T3 Code open the sessions as chats in one project folder; Orca and Conductor give each session its own git worktree, as they do by default.</li>
<li>Orca runs each session as the Claude Code terminal interface inside a shell; the others drive Claude Code without a terminal.</li>
<li>Apps stop idle agents on their own schedule: Sikemux puts an idle chat to sleep after 2 minutes unless its answer is still unread, and keeps at most 3 idle chats awake that no window shows, sleeping the oldest beyond that even when unread; T3 Code stops one after 30 minutes. The "claude running" count shows how many were alive at the end.</li>
<li>Conductor has no way to start sessions from outside, so its first session was started by hand; it opened the other ${runs[0].sessionsOpened - 1} itself with its own CreateWorkspace tool. Conductor uses the same claude binary as the others through its <code>claude_executable_path</code> setting.</li>
<li>Each app gives its agents its own tools and prompts, which changes what the agent CLIs hold.</li>
</ul>
<h2>Every process</h2>
${breakdown}
</main></body></html>
`;
writeFileSync(out, html);
console.error(out);
