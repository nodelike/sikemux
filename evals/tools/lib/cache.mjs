import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { REPO } from "./surface.mjs";
import { sikemuxSystemLines } from "./runners.mjs";
import { agentZshOptions } from "./sandbox.mjs";
import { baseCss } from "./answers.mjs";

const ROOT = join(homedir(), "Library/Caches/sikemux-tool-eval");
const RESULTS = join(ROOT, "results");
const QUEUE = join(ROOT, "queue");
const LAST_FULL = join(ROOT, "last-full.json");

const TOOL_PATHS = [
  "tools",
  "crates/sikemux-host/src/tools_mcp",
  "plugins",
  "AGENTS.md",
  "sikemux.json",
];
const HARNESS_PATHS = [
  "evals/tools/lib",
  "evals/tools/rivals",
  "evals/tools/fixtures",
];
const IGNORED = new Set([
  "evals/tools/lib/report.mjs",
  "evals/tools/lib/cache.mjs",
]);

function sha(value) {
  return createHash("sha256")
    .update(typeof value === "string" ? value : JSON.stringify(value))
    .digest("hex");
}

function git(args, input) {
  return execFileSync("git", args, {
    cwd: REPO,
    encoding: "utf8",
    input,
    maxBuffer: 64 << 20,
  }).trim();
}

function counted(path) {
  if (IGNORED.has(path)) return false;
  return (
    !path.startsWith("plugins/") ||
    /^plugins\/[^/]+\/manifest\.json$/.test(path)
  );
}

// Content ids of the files under paths, from a commit or as they are on disk now.
function blobs(paths, at) {
  if (at) {
    return git(["ls-tree", "-r", at, "--", ...paths])
      .split("\n")
      .map((line) => line.split(/\s+/))
      .filter(([, , , path]) => path && counted(path))
      .map(([, , id, path]) => `${id} ${path}`);
  }
  const files = git(["ls-files", "-co", "--exclude-standard", "--", ...paths])
    .split("\n")
    .filter((path) => path && counted(path) && existsSync(join(REPO, path)));
  const ids = git(["hash-object", "--stdin-paths"], files.join("\n")).split(
    "\n",
  );
  return files.map((path, index) => `${ids[index]} ${path}`);
}

// Everything outside a case that decides how its run goes: the tools as the agent meets them, the harness, the CLI and the limits.
export function surfaceInputs({ at, claude, shell, shellEnv, timeout, turns }) {
  return sha({
    tools: blobs(TOOL_PATHS, at),
    harness: blobs(HARNESS_PATHS, null),
    systemLines: sikemuxSystemLines(at),
    zsh: agentZshOptions(at),
    css: baseCss(),
    claude,
    shell,
    shellEnv: shellEnv ? readFileSync(shellEnv, "utf8") : null,
    timeout,
    turns,
  });
}

export function caseInputs(surface, testCase) {
  const { file, ...content } = testCase;
  return sha({ surface, content });
}

export function runKey(inputs, variant, trial) {
  return sha({
    inputs,
    model: variant.id,
    search: variant.search ?? null,
    trial: trial ?? 1,
  });
}

export function cachedRun(key, trial) {
  const path = join(RESULTS, `${key}.json`);
  if (!existsSync(path)) return null;
  const { trial: _, ...row } = JSON.parse(readFileSync(path, "utf8"));
  return { ...row, ...(trial ? { trial } : {}), cached: true };
}

// A run that timed out or failed to start says more about the machine than the model, so it runs again next time.
export function saveRun(key, row) {
  if (row.failures.some((failure) => failure.check === "run")) return;
  mkdirSync(RESULTS, { recursive: true });
  const { cached, ...fresh } = row;
  writeFileSync(join(RESULTS, `${key}.json`), JSON.stringify(fresh));
}

export function lastFull() {
  return existsSync(LAST_FULL)
    ? JSON.parse(readFileSync(LAST_FULL, "utf8"))
    : null;
}

export function saveFull(commit, inputs) {
  mkdirSync(ROOT, { recursive: true });
  writeFileSync(
    LAST_FULL,
    JSON.stringify({ date: new Date().toISOString(), commit, inputs }, null, 1),
  );
}

function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === "EPERM";
  }
}

// One eval runs at a time on this machine; later ones wait their turn in the order they arrived.
export async function takeTurn(about) {
  mkdirSync(QUEUE, { recursive: true });
  const ticket = `${String(Date.now()).padStart(15, "0")}-${process.pid}.json`;
  writeFileSync(
    join(QUEUE, `${ticket}.new`),
    JSON.stringify({ pid: process.pid, ...about }),
  );
  renameSync(join(QUEUE, `${ticket}.new`), join(QUEUE, ticket));
  process.on("exit", () => rmSync(join(QUEUE, ticket), { force: true }));
  let shown = null;
  for (;;) {
    const ahead = readdirSync(QUEUE)
      .sort()
      .filter((name) => name.endsWith(".json") && name < ticket)
      .flatMap((name) => {
        try {
          const waiting = JSON.parse(readFileSync(join(QUEUE, name), "utf8"));
          if (alive(waiting.pid)) return [waiting];
        } catch {}
        rmSync(join(QUEUE, name), { force: true });
        return [];
      });
    if (!ahead.length) return;
    if (shown !== ahead[0].pid) {
      shown = ahead[0].pid;
      console.log(
        `queued behind ${ahead.length} eval run${ahead.length > 1 ? "s" : ""}; next is pid ${shown} in ${ahead[0].repo}: ${ahead[0].args}`,
      );
    }
    await new Promise((resolve) => setTimeout(resolve, 5000));
  }
}
