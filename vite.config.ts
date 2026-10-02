import { rmSync } from "node:fs";
import { resolve } from "node:path";
import { configDefaults, defineConfig } from "vitest/config";
import type { PluginOption } from "vite";
import react from "@vitejs/plugin-react";

function pruneBundleOnlyPublicAssets(): PluginOption {
  return {
    name: "sikemux-prune-bundle-only-public-assets",
    closeBundle() {
      for (const rel of ["screenshots", ".DS_Store"]) {
        rmSync(resolve("dist", rel), { recursive: true, force: true });
      }
    },
  };
}

function chunkName(id: string): string | undefined {
  // The diff panes and the code fences in a chat both colour through
  // Shiki, and neither should pay for it until it has code to colour.
  if (id.endsWith("/src/vendor/shiki.ts")) return "highlighter";
  if (!id.includes("node_modules")) return undefined;
  const packagePath = id.slice(id.lastIndexOf("/node_modules/") + 14);
  // Grammar packages (@shikijs/langs/*) are reached only through the
  // per-language dynamic imports in src/vendor/shiki.ts, so leaving
  // them unassigned lets Rolldown split each grammar into its own
  // chunk. Every other @shikijs/* package (core, the two engines,
  // vscode-textmate) is the highlighter's static dependency graph,
  // not a per-language import, so it stays folded into it.
  if (packagePath.startsWith("@shikijs/langs/")) return undefined;
  if (
    packagePath.startsWith("@shikijs/") ||
    packagePath.startsWith("shiki/") ||
    packagePath.startsWith("oniguruma")
  ) {
    return "highlighter";
  }
  const codemirrorPackage = packagePath.startsWith("@")
    ? packagePath.split("/").slice(0, 2).join("/")
    : packagePath.split("/")[0];
  // These tiny helpers are peer dependencies used only by the
  // @codemirror/* packages above; folding them in here keeps them
  // out of the generic vendor chunk (which would otherwise create a
  // vendor <-> codemirror-core circular chunk).
  const codemirrorCorePackages = new Set([
    "@codemirror/state",
    "@codemirror/view",
    "@codemirror/language",
    "@codemirror/commands",
    "@codemirror/autocomplete",
    "@codemirror/lint",
    "@lezer/common",
    "@lezer/lr",
    "@lezer/highlight",
    "style-mod",
    "crelt",
    "w3c-keyname",
    "@marijn/find-cluster-break",
  ]);
  if (codemirrorCorePackages.has(codemirrorPackage)) {
    return "codemirror-core";
  }
  // Only the editable diff uses the merge view, so it rides in that
  // lazy chunk rather than in every editor's language pack.
  if (codemirrorPackage === "@codemirror/merge") return undefined;
  // The bare "codemirror" meta-package (basicSetup) statically pulls
  // in both @codemirror/search (langs) and the core packages above.
  // Grouping it with langs keeps the edge one-directional: langs
  // already depends on core, so this doesn't add a second direction
  // between the two chunks.
  if (
    codemirrorPackage.startsWith("@codemirror/") ||
    codemirrorPackage.startsWith("@lezer/") ||
    codemirrorPackage.startsWith("@replit/") ||
    codemirrorPackage === "codemirror"
  ) {
    return "codemirror-langs";
  }
  // Keep the opt-in renderer out of the default startup path. The
  // dynamic import in useXterm loads this chunk only when the WebGL
  // feature gate is enabled.
  if (id.includes("@xterm/addon-webgl")) return "xterm-webgl";
  // The shader runtime is fetched only when a surface in
  // src/lib/shaderField.ts asks for one, so Rolldown already splits it
  // into its own lazy chunk. Naming it here instead would make Rolldown
  // export it as a namespace object that still points at the exports it
  // tree-shook away; chunkFileNames below gives it its name.
  if (id.includes("@paper-design/shaders")) return undefined;
  if (id.includes("@xterm")) return "xterm";
  if (
    packagePath.startsWith("react/") ||
    packagePath.startsWith("react-dom/") ||
    packagePath.startsWith("scheduler/")
  ) {
    return "react";
  }
  return "vendor";
}

// Tauri expects a fixed dev port and ignores src-tauri so the Rust watcher
// owns backend rebuilds.
export default defineConfig({
  plugins: [react(), pruneBundleOnlyPublicAssets()],
  clearScreen: false,
  resolve: {
    alias: [
      { find: /^shiki$/, replacement: resolve("src/vendor/shiki.ts") },
      {
        find: /^shiki\/(wasm|engine\/oniguruma)$/,
        replacement: resolve("src/vendor/oniguruma.ts"),
      },
    ],
  },
  define: {
    Buffer: "globalThis.Buffer",
    WorkerGlobalScope: "globalThis.WorkerGlobalScope",
  },
  build: {
    // esbuild 0.25 syntax minification can remove xterm's local const-enum
    // declaration while retaining an assignment to it. The resulting production
    // bundle throws on terminal mode queries and permanently stalls xterm's write
    // queue. Terser preserves the declaration and keeps the bundle fully minified.
    minify: "terser",
    chunkSizeWarningLimit: 900,
    rolldownOptions: {
      output: {
        codeSplitting: {
          includeDependenciesRecursively: false,
          groups: [{ name: chunkName, debugName: "manual chunks" }],
        },
        chunkFileNames: (chunk) =>
          chunk.facadeModuleId?.includes("/@paper-design/shaders/")
            ? "assets/paper-shaders-[hash].js"
            : "assets/[name]-[hash].js",
      },
    },
  },
  worker: {
    format: "es",
    // With build.minify set to terser, Vite 8 leaves the worker bundle's
    // whitespace in. The worker never loads xterm, so Oxc is safe for it.
    rolldownOptions: { output: { minify: true } },
  },
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**", "**/mobile/**"] },
  },
  test: {
    environment: "jsdom",
    exclude: [...configDefaults.exclude, "mobile/**"],
    setupFiles: "./src/test/setup.ts",
    globals: false,
    coverage: {
      provider: "v8",
      include: ["src/**/*.{ts,tsx}"],
      exclude: ["src/**/*.test.{ts,tsx}", "src/test/**", "src/**/*.d.ts"],
      reporter: ["text", "html", "lcov"],
      reportsDirectory: "coverage",
      clean: true,
      thresholds: {
        statements: 70,
        branches: 64,
        functions: 65,
        lines: 73,
      },
    },
  },
});
