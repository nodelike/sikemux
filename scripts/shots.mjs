#!/usr/bin/env node
/*
 * Takes before and after pictures of a commit, its parent first, on throwaway data: no account,
 * no paired device, never the person's core or app. Both are built in one checkout kept only for
 * pictures, so the second build is incremental and the next run starts warm.
 *
 *   node scripts/shots.mjs mac <commit> <film scene>...       film/scenes/<scene>.film, its snaps
 *   node scripts/shots.mjs phone <commit> <showcase scene>... mobile/showcase/capture.mjs scenes
 *   node scripts/shots.mjs iphone <commit>                    builds both iPhone apps on the showcase's data,
 *                                                             then prints the sim calls that take each picture
 *   node scripts/shots.mjs iphone <commit> --compare  once its pictures are in <out>/before and <out>/after
 *     [--out <folder>]   default ~/Library/Caches/com.nodelike.sikemux/shots/out/<commit>
 *
 * Both sides use the commit's own scenes and data, so a scene it adds also draws the before.
 * It writes compare.html beside the pictures, each pair stacked at full width, for page_show.
 */
import { execFileSync, spawnSync } from "node:child_process";
import {
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

const { values: options, positionals } = parseArgs({
  allowPositionals: true,
  options: { out: { type: "string" }, compare: { type: "boolean" } },
});
const [platform, wanted, ...scenes] = positionals;
const PLATFORMS = {
  mac: { overlay: ["film"], shoot: shootMac },
  phone: { overlay: ["mobile/showcase"], shoot: shootPhone },
  iphone: { overlay: ["mobile/showcase"], shoot: buildIphone },
};
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
const parent = git(here, "rev-parse", "--verify", `${commit}^`);
const home = join(homedir(), "Library/Caches/com.nodelike.sikemux/shots");
const checkout = join(home, "checkout");
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

/** One shooter at a time: the checkout and its builds are shared. */
function lock() {
  mkdirSync(home, { recursive: true });
  const file = join(home, "lock");
  try {
    const holder = Number(readFileSync(file, "utf8"));
    process.kill(holder, 0);
    console.error(
      `another shooter (pid ${holder}) is taking pictures; run again once it ends`,
    );
    process.exit(1);
  } catch {}
  writeFileSync(file, String(process.pid));
  process.on("exit", () => rmSync(file, { force: true }));
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
  run("git", ["checkout", "--quiet", "--force", "--detach", rev], checkout);
  run("git", ["clean", "-fdq"], checkout);
  const overlay = PLATFORMS[platform].overlay;
  const archive = execFileSync("git", ["archive", commit, ...overlay], {
    cwd: checkout,
    maxBuffer: 1 << 30,
  });
  execFileSync("tar", ["-x", "-C", checkout], { input: archive });
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
      ],
      checkout,
      cargoPath,
    );
  say(`${side}: built in ${Math.round(built)} s`);
  const bin = join(checkout, "target/debug");
  for (const scene of scenes) {
    const work = mkdtempSync(join(tmpdir(), "sxshot"));
    try {
      run(
        join(bin, "examples/film"),
        [
          `film/scenes/${scene}.film`,
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

/** sim install takes an app only from inside the caller's project, so both are copied into its ignored build folder. */
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
  console.log(`Both apps are built. Take each picture with the sim tool, before then after:
  sim attach
  sim install ${relative(here, join(iphoneApps, "before.app"))}
  sim launch com.nodelike.sikemux.mobile.dev arguments: ["-SikemuxShowcase", "${showcaseData}", "-SikemuxShowcaseOpen", "sikemux-dev://device/${core}"]
  sim screenshot, saving the picture as ${relative(here, join(out, "before"))}/<name>.jpg
then the same with after.app into after/, and run this again with --compare for compare.html. -SikemuxShowcaseOpen takes any of the app's links, such as sikemux-dev://device/${core}/chat/<agent>; the agents are in ${showcaseData}.`);
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
  const pairs = after
    .map((name) => {
      const before = existsSync(join(out, "before", name))
        ? `<img src="before/${name}">`
        : "<p>No before: the parent draws no such picture.</p>";
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

if (options.compare) {
  compare();
  say(`wrote ${join(out, "compare.html")}`);
  process.exit(0);
}
lock();
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
for (const [side, rev] of [
  ["before", parent],
  ["after", commit],
]) {
  say(`${side}: ${rev.slice(0, 10)}`);
  moveTo(rev);
  PLATFORMS[platform].shoot(side);
}
if (platform === "iphone") {
  iphoneSteps();
} else {
  compare();
  say(`wrote ${join(out, "compare.html")}`);
}
