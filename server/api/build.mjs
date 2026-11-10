// Bundles the API into one file, so a release needs no node_modules on the server.
import { execFileSync } from "node:child_process";
import { copyFileSync } from "node:fs";
import { sentryEsbuildPlugin } from "@sentry/node/esbuild";
import { build } from "esbuild";

const version =
  process.env.SIKEMUX_VERSION ??
  execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim();

await build({
  entryPoints: ["src/main.ts"],
  outfile: "dist/main.mjs",
  bundle: true,
  platform: "node",
  target: "node22",
  format: "esm",
  sourcemap: true,
  external: ["pg-native"],
  define: { SIKEMUX_VERSION: JSON.stringify(version) },
  banner: {
    js: "import { createRequire as sikemuxCreateRequire } from 'node:module'; const require = sikemuxCreateRequire(import.meta.url);",
  },
  logLevel: "warning",
  // Instruments the bundled Hono and pg, which Sentry cannot hook once they are inside one file.
  plugins: [
    sentryEsbuildPlugin({
      telemetry: false,
      silent: true,
      sourcemaps: { disable: true },
      release: { inject: false, create: false },
    }),
  ],
});
// publish-update checks bundles against the certificate the phones trust, so it ships beside the API.
copyFileSync(
  "../../mobile/app/certs/updates-certificate.pem",
  "dist/updates-certificate.pem",
);
console.log(`Built server/api/dist/main.mjs for ${version}`);
