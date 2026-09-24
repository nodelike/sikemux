import { rmSync } from "node:fs";
import { resolve } from "node:path";
import { defineConfig } from "vitest/config";
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

// Tauri expects a fixed dev port and ignores src-tauri so the Rust watcher
// owns backend rebuilds.
export default defineConfig({
  plugins: [react(), pruneBundleOnlyPublicAssets()],
  clearScreen: false,
  resolve: {
    alias: [
      {
        find: "@pierre/theming/themes",
        replacement: resolve("src/vendor/pierreThemes.ts"),
      },
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
    rollupOptions: {
      output: {
        onlyExplicitManualChunks: true,
        manualChunks(id) {
          if (id.endsWith("/src/vendor/pierreThemes.ts")) return "diffs";
          // The highlighter is reached two ways: through @pierre/diffs for the
          // diff panes, and on its own for the code fences in a chat. Keeping
          // it out of the diffs chunk is what lets a fence colour itself
          // without also downloading a diff renderer it will never call.
          if (id.endsWith("/src/vendor/shiki.ts")) return "highlighter";
          if (!id.includes("node_modules")) return undefined;
          const packagePath = id.slice(id.lastIndexOf("/node_modules/") + 14);
          // Grammar packages (@shikijs/langs/*) are reached only through the
          // per-language dynamic imports in src/vendor/shiki.ts, so leaving
          // them unassigned lets Rollup split each grammar into its own
          // chunk. Every other @shikijs/* package (core, the two engines,
          // vscode-textmate) is the highlighter's static dependency graph,
          // not a per-language import, so it stays folded into it.
          if (packagePath.startsWith("@shikijs/langs/")) return undefined;
          if (
            id.includes("@shikijs") ||
            id.includes("/shiki@") ||
            id.includes("oniguruma")
          ) {
            return "highlighter";
          }
          if (id.includes("@pierre") || id.includes("/diff@")) {
            return "diffs";
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
          // Same reason as the renderer above: the shader runtime is fetched
          // only when a surface in src/lib/shaderField.ts asks for one, so it
          // must not ride along in the eagerly loaded vendor chunk.
          if (id.includes("@paper-design/shaders")) return "paper-shaders";
          if (id.includes("@xterm")) return "xterm";
          if (
            packagePath.startsWith("react/") ||
            packagePath.startsWith("react-dom/") ||
            packagePath.startsWith("scheduler/")
          ) {
            return "react";
          }
          return "vendor";
        },
      },
    },
  },
  worker: {
    format: "es",
  },
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  test: {
    environment: "jsdom",
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
        statements: 10,
        branches: 8,
        functions: 7,
        lines: 10,
      },
    },
  },
});
