#!/usr/bin/env node

// Builds `sikemux-notch`, the Swift helper that draws the island over the
// MacBook notch, as an app of its own: the window server plays a background
// process's trackpad haptics only when it is a real app. The macOS app carries
// it in Contents/Helpers; `--dev` puts it beside a dev build instead.

import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import { appMetadata } from "./app-metadata.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const packageDir = join(root, "helpers", "notch");
const args = process.argv.slice(2);
const name = "sikemux-notch";

function fail(message) {
  console.error(`Notch helper build failed: ${message}`);
  process.exit(1);
}

function run(command, commandArgs, options = {}) {
  const result = spawnSync(command, commandArgs, {
    cwd: root,
    encoding: "utf8",
    stdio: options.capture ? ["ignore", "pipe", "inherit"] : "inherit",
  });
  if (result.error) fail(`${command}: ${result.error.message}`);
  if (result.status !== 0)
    fail(`${command} exited with status ${result.status}`);
  return result.stdout?.trim() ?? "";
}

function option(flag) {
  const exact = args.indexOf(flag);
  if (exact >= 0) return args[exact + 1] ?? "";
  return (
    args.find((arg) => arg.startsWith(`${flag}=`))?.slice(flag.length + 1) ?? ""
  );
}

function hostTriple() {
  const details = run("rustc", ["-vV"], { capture: true });
  const host = details.match(/^host:\s*(\S+)$/m)?.[1];
  if (!host) fail("could not determine the Rust host target");
  return host;
}

const target = option("--target") || hostTriple();
if (!target.includes("apple-darwin")) {
  console.log(`- ${name} skipped: the notch is macOS only`);
  process.exit(0);
}

const archs = {
  "aarch64-apple-darwin": ["arm64"],
  "x86_64-apple-darwin": ["x86_64"],
  "universal-apple-darwin": ["arm64", "x86_64"],
}[target];
if (!archs) fail(`unsupported target ${target}`);

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
  ...archs.flatMap((arch) => ["--arch", arch]),
];
run("swift", swiftArgs);
const built = join(
  run("swift", [...swiftArgs, "--show-bin-path"], { capture: true }),
  name,
);

const dev = args.includes("--dev");
const appName = dev ? "Sikemux Notch Dev" : "Sikemux Notch";
const bundle = dev
  ? join(root, "target", "debug", `${appName}.app`)
  : join(root, "packaging", "binaries", "notch", `${appName}.app`);
const { version } = appMetadata();
rmSync(bundle, { recursive: true, force: true });
mkdirSync(join(bundle, "Contents", "MacOS"), { recursive: true });
mkdirSync(join(bundle, "Contents", "Resources", "Fonts"), { recursive: true });
writeFileSync(
  join(bundle, "Contents", "Info.plist"),
  `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>com.nodelike.sikemux.notch${dev ? ".dev" : ""}</string>
  <key>CFBundleName</key><string>${appName}</string>
  <key>CFBundleExecutable</key><string>${name}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
`,
);
const fonts = join(packageDir, "Fonts");
for (const font of [
  "Figtree_400Regular.ttf",
  "Figtree_500Medium.ttf",
  "Figtree_600SemiBold.ttf",
  "OFL.txt",
])
  copyFileSync(
    join(fonts, font),
    join(bundle, "Contents", "Resources", "Fonts", font),
  );

const destination = join(bundle, "Contents", "MacOS", name);
copyFileSync(built, destination);
chmodSync(destination, 0o755);
run("strip", ["-x", destination]);

const loadCommands = run("otool", ["-l", destination], { capture: true });
const toolchainPaths = [
  ...loadCommands.matchAll(/^\s+path (\/Applications\/\S+) \(offset \d+\)$/gm),
].map((match) => match[1]);
for (const path of new Set(toolchainPaths))
  run("install_name_tool", ["-delete_rpath", path, destination]);

// The app's signature covers what it carries, so the helper app is signed
// first, the way the bundler signs the app itself.
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

if (target === hostTriple() || target === "universal-apple-darwin") {
  const reported = run(destination, ["--version"], { capture: true });
  if (!reported.startsWith(name))
    fail(`unexpected --version output: ${reported}`);
}

console.log(`✓ ${appName} ready: ${bundle.slice(root.length + 1)}`);
