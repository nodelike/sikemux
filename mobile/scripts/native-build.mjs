import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { delimiter, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const mobile = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const native = join(mobile, 'native');
export const repo = resolve(mobile, '..');

export const PHONE_TARGETS = {
  ios: ['aarch64-apple-ios', 'aarch64-apple-ios-sim'],
  android: ['aarch64-linux-android', 'x86_64-linux-android'],
};

export function rustupPath(env = process.env) {
  const cargoBin = join(env.CARGO_HOME ?? join(homedir(), '.cargo'), 'bin');
  return { cargoBin, PATH: [cargoBin, env.PATH].filter(Boolean).join(delimiter) };
}

export function toolchainChannel() {
  const toml = readFileSync(join(repo, 'rust-toolchain.toml'), 'utf8');
  return /^channel\s*=\s*"([^"]+)"/m.exec(toml)?.[1];
}

const LIBRARY = 'libsikemux_mobile.a';
const librariesDir = {
  ios: join(native, 'SikemuxNativeFramework.xcframework'),
  android: join(native, 'android/src/main/jniLibs'),
};

export const buildCommand = {
  ios: 'pnpm native:ios:sim',
  android: 'pnpm native:android',
};

export const releaseCommand = {
  ios: 'pnpm native:ios:release',
  android: 'pnpm native:android:release',
};

export function libraries(platform) {
  const dir = librariesDir[platform];
  if (!existsSync(dir)) return [];
  return readdirSync(dir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => join(dir, entry.name, LIBRARY))
    .filter((path) => existsSync(path));
}

export function bindingsExist() {
  return existsSync(join(native, 'src/index.tsx'));
}

const recordPath = (platform) => join(native, 'build', `${platform}.json`);

export function recordBuild(platform, profile, simOnly) {
  const built = Object.fromEntries(libraries(platform).map((path) => [path, statSync(path).mtimeMs]));
  mkdirSync(join(native, 'build'), { recursive: true });
  writeFileSync(recordPath(platform), `${JSON.stringify({ profile, simOnly, libraries: built }, null, 2)}\n`);
}

/**
 * What the last build through scripts/ubrn.mjs made, or null when the libraries on
 * disk are not the ones it recorded (built some other way, or since deleted).
 */
export function readBuild(platform) {
  if (!existsSync(recordPath(platform))) return null;
  const record = JSON.parse(readFileSync(recordPath(platform), 'utf8'));
  const present = libraries(platform);
  const recorded = Object.keys(record.libraries);
  if (present.length === 0 || present.length !== recorded.length) return null;
  const unchanged = present.every((path) => record.libraries[path] === statSync(path).mtimeMs);
  return unchanged ? record : null;
}
