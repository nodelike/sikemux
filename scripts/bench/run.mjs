#!/usr/bin/env node
// Opens N Claude Code sessions in one desktop app, each answered once by a fake Messages API
// on localhost, leaves the app idle, then measures it with measure.mjs and quits it.
//
//   node scripts/bench/run.mjs <sikemux|t3code|orca|conductor|all> [--sessions 20] [--idle 600] [--out dir]
//
// The app must not be running beforehand: each run starts it fresh and quits it after.
// Sessions reach the fake API through the bench repository's .claude/settings.json, so
// every app's `claude` gets the same settings however the app starts it.
//
// Orca and Conductor write their settings into the home folder's agent configs, so they
// run only as a macOS user named `bench`, kept for this.
import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir, userInfo } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const [appName, ...rest] = process.argv.slice(2);
const options = { sessions: 20, idle: 600, interval: 30, out: join(here, "results") };
for (let i = 0; i < rest.length; i += 2) options[rest[i].replace(/^--/, "")] = rest[i + 1];
const count = Number(options.sessions);

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const say = (line) => console.error(`bench: ${line}`);
function run(command, args, extra = {}) {
  try {
    return execFileSync(command, args, { encoding: "utf8", maxBuffer: 64 << 20, ...extra }).trim();
  } catch (error) {
    error.message += `\n${error.stdout ?? ""}${error.stderr ?? ""}`;
    throw error;
  }
}
// What the apps and their agents inherit from this shell: nothing of the agent or app that
// runs the script, so a bench run never reaches the person's own Sikemux or Claude account.
const cleanEnvironment = Object.fromEntries(
  Object.entries(process.env)
    .filter(([key]) => !/^(SIKEMUX|CLAUDE|ANTHROPIC|ORCA|T3CODE|MCP|GITHUB_TOKEN|GH_TOKEN|GIT_EDITOR|ZDOTDIR)/.test(key))
    .map(([key, value]) =>
      key === "PATH" ? [key, value.split(":").filter((part) => !part.includes("com.nodelike.sikemux")).join(":")] : [key, value],
    ),
);
const prompt = (i) => `bench session ${i}: reply with one word.`;
const claudePath = run("/bin/zsh", ["-lc", "command -v claude"], { env: cleanEnvironment });
const benchUser = userInfo().username === "bench";

async function until(what, check, seconds) {
  const deadline = Date.now() + seconds * 1000;
  while (Date.now() < deadline) {
    const value = await check();
    if (value) return value;
    await sleep(1000);
  }
  throw new Error(`timed out waiting for ${what}`);
}

const running = (pattern) => {
  try {
    return run("pgrep", ["-f", pattern]).length > 0;
  } catch {
    return false;
  }
};


function claudeEnvironment(api) {
  return {
    ANTHROPIC_BASE_URL: api,
    ANTHROPIC_AUTH_TOKEN: "sk-ant-bench",
    ANTHROPIC_API_KEY: "",
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
    DISABLE_AUTOUPDATER: "1",
  };
}

function makeRepository(api) {
  mkdirSync(join(repo, ".claude"), { recursive: true });
  writeFileSync(join(repo, ".claude/settings.json"), `${JSON.stringify({ env: claudeEnvironment(api) }, null, 2)}\n`);
  writeFileSync(join(repo, "README.md"), "A repository for a memory benchmark.\n");
  const git = (...args) => run("/usr/bin/git", ["-C", repo, "-c", "user.name=bench", "-c", "user.email=bench@localhost", ...args]);
  git("init", "-q", "-b", "main");
  git("add", ".");
  git("commit", "-qm", "bench");
}

// A fresh Claude Code config folder that has finished onboarding and trusts every folder
// under the run's own, so a terminal `claude` starts its session instead of asking.
function makeClaudeHome() {
  mkdirSync(claudeHome, { recursive: true });
  writeFileSync(
    join(claudeHome, ".claude.json"),
    JSON.stringify({
      hasCompletedOnboarding: true,
      theme: "dark",
      bypassPermissionsModeAccepted: true,
      projects: { [work]: { hasTrustDialogAccepted: true }, [repo]: { hasTrustDialogAccepted: true } },
    }),
  );
}

async function sessionsAnswered(api) {
  try {
    const stats = await (await fetch(`${api}/bench/stats`)).json();
    return stats.sessions;
  } catch {
    return 0;
  }
}

const drivers = {
  sikemux: {
    bundle: "/Applications/Sikemux.app",
    async start() {
      const cli = `${this.bundle}/Contents/Helpers/Sikemux Core.app/Contents/MacOS/sikemux-host`;
      run("open", ["-g", "-a", this.bundle]);
      await until("the Sikemux core", () => {
        try {
          return run(cli, ["status", "--json"]).length > 0;
        } catch {
          return false;
        }
      }, 120);
      run(cli, ["open", repo]);
      await sleep(3000);
      say("the first `sikemux chat` outside an agent's terminal waits for Allow in the app");
      for (let i = 1; i <= count; i++) run(cli, ["chat", "start", repo, prompt(i), "--agent", "claude"]);
    },
    quit() {
      run("osascript", ["-e", 'tell application id "com.nodelike.sikemux" to quit']);
    },
  },

  t3code: {
    bundle: "/Applications/T3 Code (Nightly).app",
    env() {
      return { ...cleanEnvironment, T3CODE_HOME: join(work, "t3"), CLAUDE_CONFIG_DIR: claudeHome };
    },
    t3(args) {
      const contents = `${this.bundle}/Contents`;
      return run(`${contents}/MacOS/T3 Code (Nightly)`, [`${contents}/Resources/app.asar/apps/server/dist/bin.mjs`, ...args], {
        env: { ...this.env(), ELECTRON_RUN_AS_NODE: "1" },
      });
    },
    async start(api) {
      const env = this.env();
      const launch = ["-g", "-n"];
      for (const [key, value] of Object.entries({ T3CODE_HOME: env.T3CODE_HOME, CLAUDE_CONFIG_DIR: claudeHome, ...claudeEnvironment(api) })) {
        launch.push("--env", `${key}=${value}`);
      }
      run("open", [...launch, this.bundle]);
      await until("T3 Code's server", () => existsSync(join(env.T3CODE_HOME, "userdata/server-runtime.json")), 120);
      await sleep(3000);
      this.t3(["project", "add", repo]);
      for (let i = 1; i <= count; i++) this.t3(["thread", "start", repo, prompt(i), "--provider", "claudeAgent", "--model", "claude-opus-5-5"]);
    },
    quit() {
      run("osascript", ["-e", 'tell application "T3 Code (Nightly)" to quit']);
    },
  },

  orca: {
    bundle: "/Applications/Orca.app",
    benchUserOnly: true,
    env(api) {
      return {
        ...cleanEnvironment,
        ...claudeEnvironment(api),
        CLAUDE_CONFIG_DIR: claudeHome,
        ORCA_USER_DATA_PATH: join(work, "orca"),
        ORCA_APP_EXECUTABLE: `${this.bundle}/Contents/MacOS/Orca`,
      };
    },
    async start(api) {
      const orca = (...args) => run(`${this.bundle}/Contents/Resources/bin/orca`, [...args, "--json"], { env: this.env(api) });
      orca("open");
      await sleep(5000);
      orca("repo", "add", "--path", repo);
      for (let i = 1; i <= count; i++) {
        const made = JSON.parse(orca("worktree", "create", "--repo", `path:${repo}`, "--name", `bench-${i}`, "--agent", "claude", "--prompt", prompt(i), "--surface", "terminal", "--setup", "skip"));
        this.trust(made);
      }
    },
    trust(made) {
      const file = join(claudeHome, ".claude.json");
      const config = JSON.parse(readFileSync(file, "utf8"));
      const path = JSON.stringify(made).match(/"(?:path|worktreePath)":"([^"]+)"/)?.[1];
      if (path) config.projects[path] = { hasTrustDialogAccepted: true };
      writeFileSync(file, JSON.stringify(config));
    },
    quit() {
      run("osascript", ["-e", 'tell application "Orca" to quit']);
      try {
        run("pkill", ["-f", `${this.bundle}/Contents/Resources/app.asar.unpacked/.*daemon-entry`]);
      } catch {}
    },
  },

  // Conductor has no way to start a session from outside, so the person starts the first one;
  // the fake API then has it open the rest through Conductor's own CreateWorkspace tool.
  conductor: {
    bundle: "/Applications/Conductor.app",
    benchUserOnly: true,
    opensItsOwnSessions: true,
    async start(api) {
      const settings = join(homedir(), ".conductor/settings.managed.toml");
      mkdirSync(dirname(settings), { recursive: true });
      const env = Object.entries(claudeEnvironment(api)).map(([key, value]) => `${key} = ${JSON.stringify(value)}`);
      writeFileSync(settings, [`claude_executable_path = ${JSON.stringify(claudePath)}`, "", "[environment_variables.local]", ...env, ""].join("\n"));
      run("open", ["-a", this.bundle]);
      say(`in Conductor, add the repository ${repo} and start a workspace whose first message is:`);
      say(`  ${prompt(1)}`);
      await until("the first Conductor session", async () => (await sessionsAnswered(api)) >= 1, 1800);
    },
    quit() {
      run("osascript", ["-e", 'tell application id "com.conductor.app" to quit']);
    },
  },
};

if (appName === "all") {
  for (const name of Object.keys(drivers)) {
    if (drivers[name].benchUserOnly && !benchUser) continue;
    execFileSync(process.execPath, [fileURLToPath(import.meta.url), name, ...rest], { stdio: "inherit" });
  }
  process.exit(0);
}

const driver = drivers[appName];
if (!driver) {
  console.error(`usage: run.mjs <${Object.keys(drivers).join("|")}> [--sessions 20] [--idle 600] [--out dir]`);
  process.exit(2);
}
if (driver.benchUserOnly && !benchUser) {
  console.error(`${appName} writes into this account's agent configs; run it as the macOS user \`bench\``);
  process.exit(1);
}
if (running(`${driver.bundle}/Contents/MacOS/`)) {
  console.error(`${driver.bundle} is already running; quit it first, since a run starts it fresh and quits it after`);
  process.exit(1);
}

const work = mkdtempSync(join(tmpdir(), `bench-${appName}-`));
const repo = join(work, "repo");
const claudeHome = join(work, "claude");

const fake = spawn(process.execPath, [join(here, "fake-anthropic.mjs"), "0", driver.opensItsOwnSessions ? String(count) : "0"], {
  stdio: ["ignore", "pipe", "inherit"],
});
const api = await new Promise((resolve) => fake.stdout.once("data", (data) => resolve(data.toString().trim())));
say(`fake Messages API at ${api}, work folder ${work}`);
makeRepository(api);
makeClaudeHome();

let failed = null;
try {
  await driver.start(api);
  await until(`${count} sessions to answer`, async () => (await sessionsAnswered(api)) >= count, 600);
  say(`all ${count} sessions answered; measuring ${options.idle} s of idle`);
  mkdirSync(options.out, { recursive: true });
  const out = join(options.out, `${appName}.json`);
  execFileSync(process.execPath, [join(here, "measure.mjs"), "--app", driver.bundle, "--idle", String(options.idle), "--interval", String(options.interval), "--out", out], {
    stdio: ["ignore", "inherit", "inherit"],
  });
  const result = JSON.parse(readFileSync(out, "utf8"));
  result.sessionsOpened = count;
  result.sessionsAnswered = await sessionsAnswered(api);
  result.claude = { path: claudePath, version: run(claudePath, ["--version"], { env: cleanEnvironment }) };
  writeFileSync(out, `${JSON.stringify(result, null, 2)}\n`);
} catch (error) {
  failed = error;
} finally {
  try {
    driver.quit();
  } catch {}
  await sleep(5000);
  fake.kill();
  if (!options.keep) rmSync(work, { recursive: true, force: true });
}
if (failed) {
  console.error(failed.message);
  process.exit(1);
}
