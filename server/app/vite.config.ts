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
const WASM = join(
  repo,
  "target/wasm32-unknown-unknown/release/sikemux_markdown_wasm.wasm",
);
const TYPES: Record<string, string> = {
  ".woff2": "font/woff2",
  ".svg": "image/svg+xml",
  ".json": "application/json",
  ".wasm": "application/wasm",
};

/** The core's markdown parser for the browser, built from the same Rust. Needs rustup's cargo, which reads rust-toolchain.toml. */
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
      cwd: repo,
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

/** Files the chat view loads by address that live outside public/: the markdown parser and the app's fonts. */
function chatFiles(): Map<string, string> {
  const files = new Map<string, string>([["markdown.wasm", WASM]]);
  const fonts = join(repo, "shared/fonts");
  for (const name of readdirSync(fonts))
    if (extname(name) === ".woff2")
      files.set(`fonts/${name}`, join(fonts, name));
  return files;
}

function chatAssets(): Plugin {
  return {
    name: "sikemux-chat-assets",
    buildStart() {
      buildMarkdown();
    },
    configureServer(server) {
      buildMarkdown();
      const files = chatFiles();
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
      for (const [fileName, file] of chatFiles())
        this.emitFile({ type: "asset", fileName, source: readFileSync(file) });
    },
  };
}

export default defineConfig({
  plugins: [react(), chatAssets()],
  resolve: {
    alias: [{ find: /^@shared\//, replacement: `${join(repo, "shared")}/` }],
    dedupe: ["react", "react-dom"],
  },
  server: { port: 5173, strictPort: true, fs: { allow: [repo] } },
  build: { sourcemap: true },
});
