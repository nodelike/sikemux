#!/usr/bin/env node
/*
 * Takes pictures of a commit on throwaway data: no account, no paired device, never the person's
 * core or app. It builds in one checkout kept only for pictures, so each run after the first is
 * incremental. The "before" of each picture is the last one taken under the same scene name.
 *
 *   node scripts/shots.mjs mac <commit> <film scene>...       film/scenes/<scene>.film, its snaps
 *   node scripts/shots.mjs phone <commit> <showcase scene>... mobile/showcase/capture.mjs scenes
 *   node scripts/shots.mjs iphone <commit>                    builds the iPhone app on the showcase's data,
 *                                                             then prints the sim calls that take each picture
 *   node scripts/shots.mjs iphone <commit> --compare  once its pictures are in <out>/after
 *     [--out <folder>]   default ~/Library/Caches/com.nodelike.sikemux/shots/out/<commit>
 *
 * It writes compare.html beside the pictures, each pair stacked at full width, for page_show.
 */
import { execFileSync, spawnSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, relative, resolve } from "node:path";
import { parseArgs } from "node:util";

import { guardCheckout, SAFE_SHOOTER } from "./lib/shots-checkout.mjs";

const { values: options, positionals } = parseArgs({
  allowPositionals: true,
  options: { out: { type: "string" }, compare: { type: "boolean" } },
});
const [platform, wanted, ...scenes] = positionals;
const PLATFORMS = { mac: shootMac, phone: shootPhone, iphone: buildIphone };
if (
  !PLATFORMS[platform] ||
  !wanted ||
  (scenes.length === 0 && platform !== "iphone")
) {
  console.error(
    "usage: node scripts/shots.mjs mac|phone <commit> <scene>... | iphone <commit> [--compare] [--out <folder>]",
  );
  process.exit(2);
}

const here = resolve(import.meta.dirname, "..");
const git = (cwd, ...args) =>
  execFileSync("git", args, { cwd, encoding: "utf8" }).trim();
const store = resolve(here, git(here, "rev-parse", "--git-common-dir"));
const commit = git(here, "rev-parse", "--verify", `${wanted}^{commit}`);
const home = join(homedir(), "Library/Caches/com.nodelike.sikemux/shots");
const checkout = join(home, "checkout");
const kept = join(home, "last", platform);
const out = resolve(options.out ?? join(home, "out", commit.slice(0, 10)));

const say = (text) => console.error(`\x1b[35m›\x1b[0m ${text}`);
function run(command, args, cwd, env = {}) {
  const started = Date.now();
  const result = spawnSync(command, args, {
    cwd,
    stdio: "inherit",
    env: { ...process.env, ...env },
  });
  if (result.status === 75) {
    console.error(
      `${command} waits its turn on this Mac; run this again when Sikemux says it may start`,
    );
    process.exit(75);
  }
  if (result.status !== 0)
    throw new Error(`${command} ${args.join(" ")} failed in ${cwd}`);
  return (Date.now() - started) / 1000;
}

function running(pid) {
  try {
    return process.kill(pid, 0);
  } catch {
    return false;
  }
}

/** One shooter at a time, in order of arrival: the checkout and its builds are shared. */
function takeTurn() {
  const queue = join(home, "queue");
  const mine = `${Date.now()}-${process.pid}`;
  mkdirSync(queue, { recursive: true });
  writeFileSync(join(queue, mine), "");
  process.on("exit", () => rmSync(join(queue, mine), { force: true }));
  for (let told = false; ; told = true) {
    const ahead = readdirSync(queue)
      .filter((turn) => turn < mine)
      .filter((turn) => {
        const alive = running(Number(turn.split("-")[1]));
        if (!alive) rmSync(join(queue, turn), { force: true });
        return alive;
      });
    if (!ahead.length) return;
    if (!told)
      say(
        `waiting for the pictures checkout, ${ahead.length} ahead of this run`,
      );
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 1000);
  }
}

function moveTo(rev) {
  if (!existsSync(join(checkout, ".git"))) {
    say(
      `making the pictures checkout at ${checkout}; its first build is a cold one`,
    );
    run(
      "git",
      ["clone", "--quiet", "--shared", "--no-checkout", store, checkout],
      home,
    );
  }
  guardCheckout(checkout);
  run(
    "git",
    ["checkout", "--quiet", "--force", "--detach", rev],
    checkout,
    SAFE_SHOOTER,
  );
  if (git(checkout, "status", "--porcelain", "--untracked-files=all"))
    run("git", ["clean", "-fdq"], checkout);
  if (
    platform === "iphone" &&
    !existsSync(join(checkout, "mobile/ios/Sikemux/Core/Showcase.swift"))
  ) {
    console.error(
      `${rev.slice(0, 10)} has no iPhone showcase mode, which came in 4819b46d8; take this commit's pictures on phone instead`,
    );
    process.exit(1);
  }
}

const cargoPath = {
  PATH: `${join(checkout, "scripts/bin")}:${process.env.PATH}`,
};

/** Whether a scene signs in to the film's local accounts API, which runs from `server/`. */
const signsIn = (scene) =>
  /^(signed-in|host)\b/m.test(
    readFileSync(join(checkout, `film/scenes/${scene}.film`), "utf8"),
  );

function shootMac(side) {
  const built =
    run(
      "cargo",
      [
        "build",
        "--quiet",
        "-p",
        "sikemux-host",
        "-p",
        "sikemux-core",
        "--bin",
        "sikemux-host",
        "--bin",
        "sikemux-fake-claude",
        "--bin",
        "sikemux-fake-acp-agent",
        "--bin",
        "sikemux-fake-codex",
        "--features",
        "sikemux-core/host-addresses-file,sikemux-host/stand-in-sign-ins",
      ],
      checkout,
      cargoPath,
    ) +
    run(
      "cargo",
      [
        "build",
        "--quiet",
        "--manifest-path",
        "app/Cargo.toml",
        "--example",
        "film",
        "--features",
        "drive",
      ],
      checkout,
      cargoPath,
    ) +
    (scenes.some(signsIn)
      ? run(
          "pnpm",
          ["install", "--silent", "--frozen-lockfile", "--prefer-offline"],
          join(checkout, "server"),
        )
      : 0);
  say(`${side}: built in ${Math.round(built)} s`);
  const bin = join(checkout, "target/debug");
  for (const scene of scenes) {
    const work = mkdtempSync(join(tmpdir(), "sxshot"));
    // The film puts run/bin first on its cores' PATH. Builds older than its own stand-in `open`
    // would let a signed-out agent CLI open its sign-in page in the person's browser.
    mkdirSync(join(work, "run/bin"), { recursive: true });
    writeFileSync(join(work, "run/bin/open"), "#!/bin/sh\nexit 0\n", {
      mode: 0o755,
    });
    try {
      run(
        join(bin, "examples/film"),
        [
          join(checkout, `film/scenes/${scene}.film`),
          "--core",
          join(bin, "sikemux-host"),
          "--claude",
          join(bin, "sikemux-fake-claude"),
          "--work",
          join(work, "run"),
          "--lockup",
          "brand/wordmark/lockup-white-2048.png",
          "--world",
          "film/world.json",
          "--stills",
          "",
          "--out",
          join(out, side, scene),
        ],
        checkout,
        { SIKEMUX_CONFIG_DIR: join(work, "config") },
      );
    } finally {
      rmSync(work, { recursive: true, force: true });
    }
  }
}

function shootPhone(side) {
  const installed =
    run(
      "pnpm",
      ["install", "--silent", "--frozen-lockfile", "--prefer-offline"],
      checkout,
    ) +
    run(
      "pnpm",
      ["install", "--silent", "--frozen-lockfile", "--prefer-offline"],
      join(checkout, "mobile"),
    );
  say(`${side}: installed in ${Math.round(installed)} s`);
  run(
    "node",
    [
      "showcase/capture.mjs",
      ...scenes,
      "--port",
      "8797",
      "--out",
      join(out, side),
    ],
    join(checkout, "mobile"),
    cargoPath,
  );
}

/** sim install takes an app only from inside the caller's project, so it is copied into its ignored build folder. */
const iphoneApps = join(here, "mobile/ios/build/shots");
const showcaseData = join(out, "showcase.json");

function buildIphone(side) {
  const ios = join(checkout, "mobile/ios");
  const built =
    run(
      "pnpm",
      ["install", "--silent", "--frozen-lockfile", "--prefer-offline"],
      join(checkout, "mobile"),
    ) +
    run("scripts/build-core.sh", ["--sim-only"], ios) +
    run("xcodegen", ["generate", "--quiet"], ios) +
    run(
      "xcodebuild",
      [
        "-quiet",
        "-project",
        "Sikemux.xcodeproj",
        "-scheme",
        "Sikemux",
        "-configuration",
        "Debug",
        "-destination",
        "generic/platform=iOS Simulator",
        "-derivedDataPath",
        "build/derived",
        "ARCHS=arm64",
        "CODE_SIGNING_ALLOWED=NO",
        "build",
      ],
      ios,
    );
  say(`${side}: built in ${Math.round(built)} s`);
  if (!existsSync(showcaseData))
    run(
      "node",
      ["showcase/export.mjs", showcaseData],
      join(checkout, "mobile"),
      cargoPath,
    );
  mkdirSync(iphoneApps, { recursive: true });
  rmSync(join(iphoneApps, `${side}.app`), { recursive: true, force: true });
  run(
    "cp",
    [
      "-R",
      join(
        ios,
        "build/derived/Build/Products/Debug-iphonesimulator/Sikemux.app",
      ),
      join(iphoneApps, `${side}.app`),
    ],
    here,
  );
}

function iphoneSteps() {
  const core = JSON.parse(readFileSync(showcaseData, "utf8")).paired[0]?.core;
  console.log(`The app is built. Take each picture with the sim tool:
  sim attach
  sim install ${relative(here, join(iphoneApps, "after.app"))}
  sim launch com.nodelike.sikemux.mobile.dev arguments: ["-SikemuxShowcase", "${showcaseData}", "-SikemuxShowcaseOpen", "sikemux-dev://device/${core}"]
  sim screenshot, saving the picture as ${relative(here, join(out, "after"))}/<name>.jpg
then run this again with --compare for compare.html; a picture keeps its name across commits to have a before. -SikemuxShowcaseOpen takes any of the app's links, such as sikemux-dev://device/${core}/chat/<agent>; the agents are in ${showcaseData}.`);
}

function pictures(folder) {
  if (!existsSync(folder)) return [];
  return readdirSync(folder).flatMap((name) => {
    const path = join(folder, name);
    if (statSync(path).isDirectory()) return pictures(path);
    return /\.(png|jpg)$/.test(name) ? [path] : [];
  });
}

function compare() {
  const after = pictures(join(out, "after"))
    .map((path) => relative(join(out, "after"), path))
    .sort();
  if (!existsSync(join(out, "before"))) {
    for (const name of after) {
      if (existsSync(join(kept, name)))
        cpSync(join(kept, name), join(out, "before", name));
    }
  }
  for (const name of after) cpSync(join(out, "after", name), join(kept, name));
  const pairs = after
    .map((name) => {
      const before = existsSync(join(out, "before", name))
        ? `<img src="before/${name}">`
        : "<p>No before: no picture was taken under this name yet.</p>";
      return `<h2>${name}</h2><h3>Before</h3>${before}<h3>After</h3><img src="after/${name}">`;
    })
    .join("\n");
  const title = git(here, "log", "-1", "--format=%h %s", commit)
    .slice(0, 120)
    .replace(/[<&]/g, "");
  writeFileSync(
    join(out, "compare.html"),
    `<!doctype html><meta charset="utf-8"><title>Before and after</title>
<style>body{margin:24px;background:#111;color:#ddd;font:14px system-ui}img{width:100%;display:block}h2{margin-top:40px}h3{color:#999;font-weight:500}</style>
<h1>${title}</h1>\n${pairs}\n`,
  );
}

function trimTarget() {
  const target = join(checkout, "target");
  if (!existsSync(target)) return;
  const result = spawnSync(
    process.execPath,
    [join(here, "scripts/prune-target.mjs"), "--throttled", target],
    { cwd: here, encoding: "utf8" },
  );
  const line = result.stdout?.trim();
  if (line && !line.startsWith("Freed 0.0 GB")) say(line);
}

if (options.compare) {
  compare();
  say(`wrote ${join(out, "compare.html")}`);
  process.exit(0);
}
takeTurn();
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
say(`after: ${commit.slice(0, 10)}`);
moveTo(commit);
try {
  PLATFORMS[platform]("after");
} finally {
  trimTarget();
}
if (platform === "iphone") {
  iphoneSteps();
} else {
  compare();
  say(`wrote ${join(out, "compare.html")}`);
}
