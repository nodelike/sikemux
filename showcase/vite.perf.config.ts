import { resolve } from "node:path";
import { mergeConfig } from "vite";
import showcase from "./vite.config.ts";

// A timing run takes half a minute, and an edit anywhere in the tree would reload the page under it.
export default mergeConfig(showcase, {
  server: { hmr: false, watch: null },
  // Time the terminal renderer the app ships, not the one the screenshots use.
  define: { "import.meta.env.VITE_TERMINAL_WEBGL": JSON.stringify("1") },
  build: {
    outDir: resolve(import.meta.dirname, "out/perf"),
    emptyOutDir: true,
    rolldownOptions: { input: resolve(import.meta.dirname, "perf.html") },
  },
});
