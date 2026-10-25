import { execFileSync } from "node:child_process";
import {
  createReadStream,
  existsSync,
  readdirSync,
  readFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { extname, join } from "node:path";
import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";

const repo = fileURLToPath(new URL("../..", import.meta.url));
const MAC = `${join(repo, "src")}/`;
const mac = (path: string) => join(MAC, path);
const here = (path: string) => fileURLToPath(new URL(path, import.meta.url));

/** The Mac modules a browser cannot run, and what stands in for each on a shared chat's page. */
const STAND_INS = new Map([
  [mac("state/store.ts"), here("src/share/standins/store.ts")],
  [mac("state/commands/index.ts"), here("src/share/standins/commands.ts")],
  [mac("rail/FileTree.tsx"), here("src/share/standins/fileTree.tsx")],
  [mac("api/markdown.ts"), here("src/share/standins/markdown.ts")],
]);
const TAURI = new Map([
  ["@tauri-apps/api/core", here("src/share/standins/tauri-core.ts")],
  ["@tauri-apps/api/event", here("src/share/standins/tauri-event.ts")],
]);

function macStandIns(): Plugin {
  return {
    name: "sikemux-mac-stand-ins",
    enforce: "pre",
    async resolveId(source, importer, options) {
      const tauri = TAURI.get(source);
      if (tauri) return tauri;
      if (!importer?.startsWith(MAC) || !source.startsWith(".")) return null;
      const resolved = await this.resolve(source, importer, {
        ...options,
        skipSelf: true,
      });
      return (resolved && STAND_INS.get(resolved.id)) ?? null;
    },
  };
}

const WASM = join(
  repo,
  "src-tauri/target/wasm32-unknown-unknown/release/sikemux_markdown_wasm.wasm",
);
const TYPES: Record<string, string> = {
  ".woff2": "font/woff2",
  ".svg": "image/svg+xml",
  ".json": "application/json",
  ".wasm": "application/wasm",
};

/** The Mac's markdown parser for the browser, built from the same Rust. Needs rustup's cargo, which reads rust-toolchain.toml. */
function buildMarkdown() {
  const rustup = join(homedir(), ".cargo/bin");
  execFileSync(
    "cargo",
    [
      "build",
      "-p",
      "sikemux-markdown-wasm",
      "--target",
      "wasm32-unknown-unknown",
      "--release",
    ],
    {
      cwd: join(repo, "src-tauri"),
      stdio: "inherit",
      env: {
        ...process.env,
        PATH: existsSync(rustup)
          ? `${rustup}:${process.env.PATH ?? ""}`
          : process.env.PATH,
      },
    },
  );
}

/** Files the Mac's chat view loads by address, served from where the Mac keeps them. */
function macFiles(): Map<string, string> {
  const files = new Map<string, string>([["markdown.wasm", WASM]]);
  for (const folder of ["fonts", "file-icons"])
    for (const name of readdirSync(join(repo, "public", folder)))
      if ([".woff2", ".svg", ".json"].includes(extname(name)))
        files.set(`${folder}/${name}`, join(repo, "public", folder, name));
  return files;
}

function macAssets(): Plugin {
  return {
    name: "sikemux-mac-assets",
    buildStart() {
      buildMarkdown();
    },
    configureServer(server) {
      buildMarkdown();
      const files = macFiles();
      server.middlewares.use((request, response, next) => {
        const path = new URL(request.url ?? "/", "http://app").pathname.slice(
          1,
        );
        const file = files.get(path);
        if (!file) return next();
        response.setHeader(
          "content-type",
          TYPES[extname(file)] ?? "application/octet-stream",
        );
        createReadStream(file).pipe(response);
      });
    },
    generateBundle() {
      for (const [fileName, file] of macFiles())
        this.emitFile({ type: "asset", fileName, source: readFileSync(file) });
    },
  };
}

export default defineConfig({
  plugins: [react(), macStandIns(), macAssets()],
  resolve: {
    alias: [
      { find: /^@mac\//, replacement: MAC },
      { find: /^@shared\//, replacement: `${join(repo, "shared")}/` },
    ],
    dedupe: ["react", "react-dom"],
  },
  server: { port: 5173, strictPort: true, fs: { allow: [repo] } },
  build: { sourcemap: true },
});
