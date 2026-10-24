import { execFileSync, spawn } from "node:child_process";
import { existsSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";
import type { IncomingMessage, ServerResponse } from "node:http";
import type { Browser } from "playwright-core";
import {
  mergeConfig,
  type PluginOption,
  type PreviewServer,
  type ViteDevServer,
} from "vite";
import base from "../vite.config.ts";
import { DEMO_PROJECTS, PANE_IMAGE } from "./world/projects.ts";
import { PANE_IMAGE_FILE } from "./world/paneImage.ts";
import { STAGE } from "./stage.mjs";

/** A still is 4320 pixels wide, so each of the thirds layout's images is 1440. */
const STILL_WIDTH = 4320;

const LOCAL_ROOTS: Record<string, string> = {
  sikemux: resolve(import.meta.dirname, ".."),
  "sikemux-front": join(homedir(), "projects/personal/sikemux-front"),
  "moodboard-studio": join(homedir(), "projects/personal/moodboard-studio"),
};

const WALLPAPER_FILE = join(homedir(), "wallpaper/midnight-garden.jpeg");

const MAX_FILE_BYTES = 400_000;
const HIDDEN_FOLDERS = new Set([
  ".claude",
  ".scratch",
  "scratch",
  "graphify-out",
  "coverage",
]);

interface ProjectFiles {
  root: string;
  files: Set<string>;
  dirs: Map<string, { name: string; isDir: boolean }[]>;
}

function localRoot(
  demoPath: string,
): { project: ProjectFiles; relative: string } | null {
  for (const project of DEMO_PROJECTS) {
    if (demoPath !== project.path && !demoPath.startsWith(`${project.path}/`))
      continue;
    return {
      project: projectFiles(project.name),
      relative: demoPath.slice(project.path.length + 1),
    };
  }
  return null;
}

const indexed = new Map<string, ProjectFiles>();

// Only tracked files are visible, so secrets, scratch folders and other people's work in progress never reach a screenshot.
function projectFiles(name: string): ProjectFiles {
  const cached = indexed.get(name);
  if (cached) return cached;
  const root = LOCAL_ROOTS[name];
  const listed =
    !root || !existsSync(root)
      ? ""
      : execFileSync("git", ["ls-files", "--cached"], {
          cwd: root,
          encoding: "utf8",
          maxBuffer: 64 << 20,
        })
          .split("\n")
          .filter(
            (path) =>
              path &&
              !path
                .split("/")
                .some(
                  (part) => part.startsWith(".env") || HIDDEN_FOLDERS.has(part),
                ),
          );
  const files = new Set(listed);
  const dirs = new Map<string, { name: string; isDir: boolean }[]>();
  const add = (dir: string, entry: string, isDir: boolean) => {
    const entries = dirs.get(dir) ?? [];
    if (!entries.some((existing) => existing.name === entry))
      entries.push({ name: entry, isDir });
    dirs.set(dir, entries);
  };
  for (const file of files) {
    const parts = file.split("/");
    for (let depth = 0; depth < parts.length; depth++) {
      add(
        parts.slice(0, depth).join("/"),
        parts[depth],
        depth < parts.length - 1,
      );
    }
  }
  const project = { root, files, dirs };
  indexed.set(name, project);
  return project;
}

function readDir(demoPath: string) {
  const found = localRoot(demoPath);
  const entries = found?.project.dirs.get(found.relative) ?? null;
  if (!found || !entries)
    return { path: demoPath, entries: [], error: "not found" };
  return {
    path: demoPath,
    entries: entries.map((entry) => ({
      name: entry.name,
      path: `${demoPath}/${entry.name}`,
      is_dir: entry.isDir,
    })),
    error: null,
  };
}

function readFile(demoPath: string): string | null {
  const found = localRoot(demoPath);
  if (!found || !found.project.files.has(found.relative)) return null;
  const path = join(found.project.root, found.relative);
  if (statSync(path).size > MAX_FILE_BYTES) return null;
  return readFileSync(path, "utf8");
}

function shortAge(relative: string): string {
  const [amount, unit] = relative.split(" ");
  const letter = unit.startsWith("mo") ? "mo" : unit[0];
  return `${amount}${letter} ago`;
}

function gitLog(name: string, count: number, head = "HEAD") {
  if (!LOCAL_ROOTS[name] || !existsSync(LOCAL_ROOTS[name])) return [];
  const format = ["%H", "%P", "%an", "%ar", "%s", "%D"].join("%x1f");
  const out = execFileSync(
    "git",
    ["log", `-n${count}`, `--format=${format}`, "--end-of-options", head],
    {
      cwd: LOCAL_ROOTS[name],
      encoding: "utf8",
    },
  );
  return out
    .split("\n")
    .filter(Boolean)
    .map((line, index) => {
      const [full, parents, author, age, subject, decorations] =
        line.split("\x1f");
      const refs = decorations
        .split(", ")
        .filter(Boolean)
        .map((ref) => ref.replace(/^HEAD -> /, ""))
        .filter((ref) => ref !== "origin/HEAD");
      if (index === 0) refs.unshift("HEAD");
      return {
        hash: full.slice(0, 7),
        full_hash: full,
        parents: parents.split(" ").filter(Boolean),
        author,
        author_email: "",
        date: shortAge(age),
        subject,
        refs,
        unpushed: index < 2,
      };
    });
}

// Markdown is read by the app's Rust parser, run as a small process that answers one line per request.
let markdownParser: {
  write: (line: string) => void;
  stop: () => void;
  waiting: ((line: string) => void)[];
} | null = null;

function parseMarkdown(requests: unknown[]): Promise<unknown[]> {
  if (!markdownParser) {
    const child = spawn(
      "cargo",
      [
        "run",
        "--quiet",
        "--manifest-path",
        resolve(import.meta.dirname, "../src-tauri/Cargo.toml"),
        "-p",
        "sikemux-markdown",
        "--example",
        "stdio",
      ],
      { stdio: ["pipe", "pipe", "inherit"] },
    );
    const waiting: ((line: string) => void)[] = [];
    createInterface({ input: child.stdout }).on("line", (line) =>
      waiting.shift()?.(line),
    );
    markdownParser = {
      write: (line) => child.stdin.write(`${line}\n`),
      stop: () => child.kill(),
      waiting,
    };
  }
  const parser = markdownParser;
  return Promise.all(
    requests.map(
      (request) =>
        new Promise((resolve) => {
          parser.waiting.push((line) => resolve(JSON.parse(line)));
          parser.write(JSON.stringify(request));
        }),
    ),
  );
}

// Chats are folded by the core's own fold, run as a small process that answers one line per request.
let chatFold: {
  write: (line: string) => void;
  stop: () => void;
  waiting: ((line: string) => void)[];
} | null = null;

function foldChat(request: unknown): Promise<unknown> {
  if (!chatFold) {
    const child = spawn(
      "cargo",
      [
        "run",
        "--quiet",
        "--manifest-path",
        resolve(import.meta.dirname, "../src-tauri/Cargo.toml"),
        "-p",
        "sikemux-fold",
        "--example",
        "stdio",
      ],
      { stdio: ["pipe", "pipe", "inherit"] },
    );
    const waiting: ((line: string) => void)[] = [];
    createInterface({ input: child.stdout }).on("line", (line) =>
      waiting.shift()?.(line),
    );
    chatFold = {
      write: (line) => child.stdin.write(`${line}\n`),
      stop: () => child.kill(),
      waiting,
    };
  }
  const fold = chatFold;
  return new Promise((resolve) => {
    fold.waiting.push((line) => resolve(JSON.parse(line)));
    fold.write(JSON.stringify(request));
  });
}

// Xcode's bezel for the demo iPhone comes from the installed simulator helper, which only
// draws it for a booted device, so the first answer is kept in showcase/out for later runs.
const SIM_CHROME = join(import.meta.dirname, "out/simulator-chrome.json");
const SIM_HELPER = join(
  homedir(),
  "Library/Application Support/com.nodelike.sikemux/sim/sikemux-sim",
);

async function simulatorChrome(): Promise<unknown> {
  if (existsSync(SIM_CHROME))
    return JSON.parse(readFileSync(SIM_CHROME, "utf8"));
  if (!existsSync(SIM_HELPER)) return {};
  const listed = JSON.parse(
    execFileSync("xcrun", ["simctl", "list", "devices", "booted", "-j"], {
      encoding: "utf8",
    }),
  ) as {
    devices: Record<string, { udid: string; name: string }[]>;
  };
  const phone = Object.values(listed.devices)
    .flat()
    .find((device) => device.name.startsWith("iPhone"));
  if (!phone) return {};
  const helper = spawn(SIM_HELPER, [], { stdio: ["pipe", "pipe", "ignore"] });
  const answers = new Map<number, Record<string, unknown>>();
  await new Promise<void>((settle) => {
    createInterface({ input: helper.stdout }).on("line", (line) => {
      const answer = JSON.parse(line) as Record<string, unknown>;
      answers.set(answer.id as number, answer);
      if (answers.size === 2) settle();
    });
    helper.stdin.write(
      `${JSON.stringify({ id: 1, type: "chrome", udid: phone.udid })}\n`,
    );
    helper.stdin.write(
      `${JSON.stringify({ id: 2, type: "mask", udid: phone.udid })}\n`,
    );
    setTimeout(settle, 15_000);
  });
  helper.kill();
  const found = {
    chrome: answers.get(1)?.chrome ?? null,
    mask: answers.get(2)?.mask ?? null,
  };
  if (found.chrome) writeFileSync(SIM_CHROME, JSON.stringify(found));
  return found;
}

function gitShow(name: string, rev: string, path: string): string {
  if (!LOCAL_ROOTS[name] || !existsSync(LOCAL_ROOTS[name])) return "";
  try {
    return execFileSync("git", ["show", `${rev}:${path}`], {
      cwd: LOCAL_ROOTS[name],
      encoding: "utf8",
      maxBuffer: 16 << 20,
    });
  } catch {
    return "";
  }
}

type DiffRow = [0 | 1 | 2, number, string] | [3, number, ""];

/** A real commit's change to one file, in the rows the app's diff view reads. */
function fileDiff(
  name: string,
  path: string,
  base: string,
  head: string,
  full: boolean,
): DiffRow[] {
  if (!LOCAL_ROOTS[name] || !existsSync(LOCAL_ROOTS[name])) return [];
  const out = execFileSync(
    "git",
    ["diff", "--no-color", `-U${full ? 100_000 : 3}`, base, head, "--", path],
    { cwd: LOCAL_ROOTS[name], encoding: "utf8", maxBuffer: 16 << 20 },
  );
  const rows: DiffRow[] = [];
  let oldLine = 0;
  let newLine = 0;
  let shown = 0;
  for (const line of out.split("\n")) {
    const hunk = line.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
    if (hunk) {
      oldLine = Number(hunk[1]);
      newLine = Number(hunk[2]);
      if (newLine - 1 > shown) rows.push([3, newLine - 1 - shown, ""]);
      continue;
    }
    if (!oldLine && !newLine) continue;
    if (line.startsWith("+")) rows.push([1, newLine++, line.slice(1)]);
    else if (line.startsWith("-")) rows.push([2, oldLine++, line.slice(1)]);
    else if (line.startsWith(" ")) {
      rows.push([0, newLine++, line.slice(1)]);
      oldLine++;
    } else continue;
    shown = newLine - 1;
  }
  const length = gitShow(name, head, path).split("\n").length - 1;
  if (length > shown) rows.push([3, length - shown, ""]);
  return rows;
}

function commitFiles(name: string, rev: string): string[] {
  if (!LOCAL_ROOTS[name] || !existsSync(LOCAL_ROOTS[name])) return [];
  try {
    return execFileSync(
      "git",
      ["show", "--name-only", "--format=", "--end-of-options", rev],
      { cwd: LOCAL_ROOTS[name], encoding: "utf8" },
    )
      .split("\n")
      .filter(Boolean);
  } catch {
    return [];
  }
}

async function body(
  request: IncomingMessage,
): Promise<Record<string, unknown>> {
  const chunks: Buffer[] = [];
  for await (const chunk of request) chunks.push(chunk as Buffer);
  return JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
}

function send(response: ServerResponse, status: number, value: unknown) {
  response.statusCode = status;
  response.setHeader("Content-Type", "application/json");
  response.end(JSON.stringify(value));
}

let chosenWallpaper: Buffer | null = null;
let chosenScreenshot: Buffer | null = null;

let snapshotBrowser: Promise<Browser> | null = null;

const NOTCH_DIR = join(import.meta.dirname, "out/notch");
let notchRender: Promise<void> | null = null;

// The island is SwiftUI, so its own snapshot tool draws it once per server, on a clear background,
// sharp enough for a video to zoom right in on it, about the demo's own agents.
// Pictures already drawn from the current fixture are kept, so a capture does not rebuild the island.
function renderNotch(): Promise<void> {
  const fixture = join(import.meta.dirname, "world/notch.fixture");
  const drawn = join(NOTCH_DIR, "P1-permission.png");
  if (existsSync(drawn) && statSync(drawn).mtimeMs > statSync(fixture).mtimeMs)
    return Promise.resolve();
  notchRender ??= new Promise((done, fail) => {
    const notchPackage = join(import.meta.dirname, "../src-tauri/notch");
    spawn(
      "swift",
      [
        "run",
        "--package-path",
        notchPackage,
        "notch-snapshots",
        NOTCH_DIR,
        "--bare",
        "--scale",
        "4",
        "--fixture",
        fixture,
      ],
      {
        stdio: "ignore",
      },
    ).on("exit", (code) => {
      if (code === 0) return done();
      notchRender = null;
      fail(new Error(`notch-snapshots exited with ${code}`));
    });
  });
  return notchRender;
}

async function snapshot(
  origin: string,
  { state, layout, screenshot, notch }: Record<string, unknown>,
): Promise<string[]> {
  snapshotBrowser ??= import("playwright-core").then(({ chromium }) =>
    chromium.launch({ channel: "chrome" }),
  );
  const context = await (
    await snapshotBrowser
  ).newContext({
    viewport: STAGE,
    deviceScaleFactor: STILL_WIDTH / STAGE.width,
    colorScheme: "dark",
  });
  try {
    await context.clock.setFixedTime(new Date("2026-09-26T09:41:00"));
    const page = await context.newPage();
    const island = notch ? `&notch=${notch}` : "";
    if (screenshot) {
      await page.goto(
        `${origin}/showcase/twitter.html?capture&screenshot${island}`,
      );
      await page.waitForFunction(() =>
        [...document.querySelectorAll<HTMLImageElement>(".shot")].every(
          (image) => image.complete && image.naturalWidth > 0,
        ),
      );
    } else {
      await page.goto(`${origin}/showcase/twitter.html?capture${island}`);
      await page.frameLocator("iframe").locator(".shell").waitFor();
      const app = page.frames()[1];
      await app.waitForFunction(() => "showcase" in window);
      await app.evaluate((restored) => {
        const { showcase } = window as unknown as {
          showcase: {
            store: { setState: (state: object) => void };
            backend: { stepLive: () => number };
          };
        };
        showcase.store.setState(restored as object);
        while (showcase.backend.stepLive() >= 0);
      }, state);
      await page.waitForTimeout(1500);
    }
    await page.waitForFunction(() => {
      const image = document.querySelector<HTMLImageElement>(".notch");
      return (
        !image?.getAttribute("src") ||
        (image.complete && image.naturalWidth > 0)
      );
    });
    const stamp = new Date().toLocaleString("sv").replace(/[ :]/g, "-");
    if (layout !== "thirds") {
      const name = `sikemux-${stamp}.png`;
      await page.screenshot({ path: join(homedir(), "Downloads", name) });
      return [name];
    }
    const names = [1, 2, 3].map((slice) => `sikemux-${stamp}-${slice}.png`);
    for (const [index, name] of names.entries()) {
      await page.screenshot({
        path: join(homedir(), "Downloads", name),
        clip: {
          x: (index * STAGE.width) / 3,
          y: 0,
          width: STAGE.width / 3,
          height: STAGE.height,
        },
      });
    }
    return names;
  } finally {
    await context.close();
  }
}

function serveDemoFiles(server: ViteDevServer | PreviewServer, port: number) {
  server.httpServer?.on("close", () => {
    markdownParser?.stop();
    chatFold?.stop();
    void snapshotBrowser?.then((browser) => browser.close());
  });
  server.middlewares.use("/__showcase", async (request, response) => {
    if (request.url === `/preview/${encodeURIComponent(PANE_IMAGE)}`) {
      if (!existsSync(PANE_IMAGE_FILE)) {
        response.statusCode = 404;
        return response.end();
      }
      response.setHeader("Content-Type", "image/jpeg");
      return response.end(readFileSync(PANE_IMAGE_FILE));
    }
    if (request.url === "/simulator/chrome") {
      response.setHeader("Content-Type", "application/json");
      return response.end(JSON.stringify(await simulatorChrome()));
    }
    // The phone showcase's screens, the permission card unless a screen is named.
    const screen = request.url?.match(/^\/simulator\/frame(?:\/([\w-]+))?$/);
    if (screen) {
      const frame = join(
        import.meta.dirname,
        `../mobile/showcase/out/${screen[1] ?? "asking"}.png`,
      );
      if (!existsSync(frame)) return response.writeHead(404).end();
      response.setHeader("Content-Type", "image/png");
      return response.end(readFileSync(frame));
    }
    const notchState = request.url?.match(/^\/notch\/([\w-]+)\.png$/);
    if (notchState) {
      await renderNotch();
      const file = join(NOTCH_DIR, `${notchState[1]}.png`);
      if (!existsSync(file)) return response.writeHead(404).end();
      response.setHeader("Content-Type", "image/png");
      return response.end(readFileSync(file));
    }
    const shownPage = request.url?.match(/^\/page\/([0-9a-f]{32})$/);
    if (shownPage) {
      response.setHeader("Content-Type", "text/html");
      return response.end(
        readFileSync(
          join(import.meta.dirname, "pages", `${shownPage[1]}.html`),
        ),
      );
    }
    if (request.url?.split("?")[0] === "/screenshot") {
      if (request.method === "POST") {
        const chunks: Buffer[] = [];
        for await (const chunk of request) chunks.push(chunk as Buffer);
        chosenScreenshot = Buffer.concat(chunks);
        return response.end();
      }
      if (!chosenScreenshot) return response.writeHead(404).end();
      return response.end(chosenScreenshot);
    }
    if (request.url?.split("?")[0] === "/wallpaper") {
      if (request.method === "POST") {
        const chunks: Buffer[] = [];
        for await (const chunk of request) chunks.push(chunk as Buffer);
        chosenWallpaper = Buffer.concat(chunks);
        return response.end();
      }
      if (!chosenWallpaper && !existsSync(WALLPAPER_FILE)) {
        response.statusCode = 404;
        return response.end();
      }
      return response.end(chosenWallpaper ?? readFileSync(WALLPAPER_FILE));
    }
    try {
      const input = await body(request);
      switch (request.url) {
        case "/read_dirs":
          return send(response, 200, (input.paths as string[]).map(readDir));
        case "/read_file": {
          const content = readFile(input.path as string);
          return content === null
            ? send(response, 404, {
                error: "No such file or directory (os error 2)",
              })
            : send(response, 200, content);
        }
        case "/list_files": {
          const found = localRoot(input.repo as string);
          return send(
            response,
            200,
            found ? [...found.project.files].sort() : [],
          );
        }
        case "/commit_files":
          return send(
            response,
            200,
            commitFiles(input.project as string, input.rev as string),
          );
        case "/git_log":
          return send(
            response,
            200,
            gitLog(
              input.project as string,
              Number(input.count ?? 60),
              input.head as string | undefined,
            ),
          );
        case "/file_diff":
          return send(
            response,
            200,
            fileDiff(
              input.project as string,
              input.path as string,
              input.base as string,
              input.head as string,
              Boolean(input.full),
            ),
          );
        case "/file_at":
          return send(
            response,
            200,
            gitShow(
              input.project as string,
              input.rev as string,
              input.path as string,
            ),
          );
        case "/snapshot":
          return send(
            response,
            200,
            await snapshot(`http://localhost:${port}`, input),
          );
        case "/markdown":
          return send(
            response,
            200,
            await parseMarkdown(input.requests as unknown[]),
          );
        case "/fold":
          return send(response, 200, await foldChat(input));
        default:
          return send(response, 404, {
            error: "unknown showcase endpoint",
          });
      }
    } catch (error) {
      return send(response, 500, { error: String(error) });
    }
  });
}

function demoFileSystem(): PluginOption {
  return {
    name: "sikemux-showcase-fs",
    configureServer: (server) =>
      serveDemoFiles(server, server.config.server.port),
    configurePreviewServer: (server) =>
      serveDemoFiles(server, server.config.preview.port),
  };
}

// A Sikemux browser tab already holds Tauri's own read-only globals, so the mocks move to globals of their own.
const TAURI_GLOBAL =
  /\bwindow\.__TAURI_(INTERNALS|EVENT_PLUGIN_INTERNALS)__\b/g;

const ownTauriGlobals = {
  name: "sikemux-showcase-own-tauri-globals",
  transform(code: string) {
    if (!code.includes("__TAURI_")) return null;
    return {
      code: code.replace(TAURI_GLOBAL, "window.__SHOWCASE_TAURI_$1__"),
      map: null,
    };
  },
};

export default mergeConfig(base, {
  plugins: [demoFileSystem(), ownTauriGlobals],
  optimizeDeps: { rolldownOptions: { plugins: [ownTauriGlobals] } },
  cacheDir: resolve(import.meta.dirname, "../node_modules/.vite-showcase"),
  server: { port: 1471, strictPort: true },
});
