#!/usr/bin/env node

// Builds `sikemux-notch`, the Swift helper that draws the island over the
// MacBook notch, as an app of its own: the window server plays a background
// process's trackpad haptics only when it is a real app. The macOS app carries
// it in Contents/Helpers; `--dev` puts it beside a dev build instead, and
// `--snapshots` also builds `notch-snapshots`, which draws the island's states.

import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  rmSync,
  writeFileSync,
  readdirSync,
  readFileSync,
} from "node:fs";
import { join } from "node:path";

import { appMetadata } from "./app-metadata.mjs";
import { building, root } from "./lib/build.mjs";

const { fail, run, hostTriple, dropToolchainPaths, checkStarts } =
  building("Notch helper");

const packageDir = join(root, "helpers", "notch");
const args = process.argv.slice(2);
const name = "sikemux-notch";

const target = hostTriple();
if (!target.includes("apple-darwin")) {
  console.log(`- ${name} skipped: the notch is macOS only`);
  process.exit(0);
}

if (target !== "aarch64-apple-darwin")
  fail(`${name} needs a Mac with Apple Silicon, not ${target}`);

run("node", [join(root, "scripts", "generate-notch-icons.mjs"), "--check"]);
run("node", [join(root, "scripts", "generate-notch-sources.mjs"), "--check"]);

const swiftArgs = [
  "build",
  "-c",
  "release",
  "--package-path",
  packageDir,
  "--product",
  name,
  "-Xswiftc",
  "-Osize",
  "--arch",
  "arm64",
];
run("swift", swiftArgs);
if (args.includes("--snapshots")) {
  // The snapshot tool builds the core's types by hand, so a new field breaks it; checks build it to catch that.
  // A release build of it cannot see NotchKit's module, so it builds as a debug build.
  run("swift", [
    "build",
    "--package-path",
    packageDir,
    "--product",
    "notch-snapshots",
  ]);
}
const built = join(
  run("swift", [...swiftArgs, "--show-bin-path"], { capture: true }),
  name,
);

const dev = args.includes("--dev");
const appName = dev ? "Sikemux Notch Dev" : "Sikemux Notch";
const bundle = dev
  ? join(root, "target", "debug", `${appName}.app`)
  : join(root, "packaging", "binaries", "notch", `${appName}.app`);
const { version, identifier } = appMetadata();
// Only builds the release workflow publishes send crashes, and only those it gives the address.
const sentryDsn = dev ? "" : (process.env.SIKEMUX_NOTCH_SENTRY_DSN ?? "");
const sentryEnvironment = process.env.SIKEMUX_USAGE_REPORTING
  ? "production"
  : "dev";
const minimumMacos = readFileSync(
  join(packageDir, "Package.swift"),
  "utf8",
).match(/\.macOS\(\.v(\d+)\)/)[1];
rmSync(bundle, { recursive: true, force: true });
mkdirSync(join(bundle, "Contents", "MacOS"), { recursive: true });
mkdirSync(join(bundle, "Contents", "Resources", "Fonts"), { recursive: true });
writeFileSync(
  join(bundle, "Contents", "Info.plist"),
  `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>${identifier}.notch${dev ? ".dev" : ""}</string>
  <key>CFBundleName</key><string>${appName}</string>
  <key>CFBundleExecutable</key><string>${name}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>${minimumMacos}.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>${
    sentryDsn
      ? `
  <key>SikemuxSentryDSN</key><string>${sentryDsn}</string>
  <key>SikemuxSentryEnvironment</key><string>${sentryEnvironment}</string>`
      : ""
  }
</dict>
</plist>
`,
);
const fonts = join(packageDir, "Fonts");
for (const font of readdirSync(fonts))
  copyFileSync(
    join(fonts, font),
    join(bundle, "Contents", "Resources", "Fonts", font),
  );

const destination = join(bundle, "Contents", "MacOS", name);
copyFileSync(built, destination);
chmodSync(destination, 0o755);
if (!dev) {
  const symbols = join(root, "target", "notch", `${name}.dSYM`);
  rmSync(symbols, { recursive: true, force: true });
  run("dsymutil", [destination, "-o", symbols]);
}
run("strip", ["-x", destination]);

dropToolchainPaths(destination);

// The app's signature covers what it carries, so the helper app is signed first.
const identity = process.env.APPLE_SIGNING_IDENTITY || "-";
run("codesign", [
  "--force",
  "--options",
  "runtime",
  ...(identity === "-" ? [] : ["--timestamp"]),
  "--sign",
  identity,
  bundle,
]);

checkStarts(destination, name);

console.log(`✓ ${appName} ready: ${bundle.slice(root.length + 1)}`);
