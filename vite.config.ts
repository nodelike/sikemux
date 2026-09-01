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
          if (
            id.endsWith("/src/vendor/shiki.ts") ||
            id.endsWith("/src/vendor/pierreThemes.ts")
          ) {
            return "diffs";
          }
          if (!id.includes("node_modules")) return undefined;
          const packagePath = id.slice(id.lastIndexOf("/node_modules/") + 14);
          if (
            id.includes("@pierre") ||
            id.includes("@shikijs") ||
            id.includes("/shiki@") ||
            id.includes("/diff@") ||
            id.includes("oniguruma")
          ) {
            return "diffs";
          }
          if (
            id.includes("@codemirror") ||
            id.includes("@lezer") ||
            id.includes("codemirror")
          ) {
            return "codemirror";
          }
          // Keep the opt-in renderer out of the default startup path. The
          // dynamic import in useXterm loads this chunk only when the WebGL
          // feature gate is enabled.
          if (id.includes("@xterm/addon-webgl")) return "xterm-webgl";
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
