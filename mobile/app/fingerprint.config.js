const { readFileSync, readdirSync } = require('node:fs');
const { join } = require('node:path');

const { rustClientSources } = require('./scripts/rust-sources');

// The runtime version is this fingerprint: an over-the-air update only reaches builds with the native code
// it was made for. The Rust client is native code too, so what it links counts, and nothing the desktop app
// alone builds with: a desktop release must not strand every installed phone on an old runtime.
const RUST = rustClientSources(join(__dirname, '../..'));

// mobile/native is generated from the Rust sources above, differently on each machine, and is only linked once
// it has been built, so the fingerprint counts its own few files instead and drops it from the linked modules.
const NATIVE = join(__dirname, '../native');
const NATIVE_FILES = [
  'package.json',
  'ubrn.config.yaml',
  ...readdirSync(join(NATIVE, 'scripts'))
    .sort()
    .map((name) => `scripts/${name}`),
];
const LINKED_MODULES = ['rncoreAutolinkingConfig:android', 'rncoreAutolinkingConfig:ios'];

/** @type {import('expo/fingerprint').Config} */
module.exports = {
  sourceSkips: ['ExpoConfigVersions', 'PackageJsonAndroidAndIosScriptsIfNotContainRun'],
  extraSources: [
    { type: 'file', filePath: '../../rust-toolchain.toml', reasons: ['rustClient'] },
    ...RUST.crates.map((dir) => ({ type: 'dir', filePath: `../../crates/${dir}`, reasons: ['rustClient'] })),
    { type: 'contents', id: 'rust/locked', contents: RUST.locked, reasons: ['rustClient'] },
    { type: 'contents', id: 'rust/workspace', contents: RUST.settings, reasons: ['rustClient'] },
    ...NATIVE_FILES.map((name) => ({
      type: 'contents',
      id: `native/${name}`,
      contents: readFileSync(join(NATIVE, name)),
      reasons: ['rustClient'],
    })),
  ],
  ignorePaths: ['**/target/**/*', '../native/**/*'],
  fileHookTransform(source, chunk) {
    if (source.type !== 'contents' || chunk == null) return chunk;
    // Nightly and stable builds of the same code share a runtime version, so an update promoted to stable still fits.
    if (source.id === 'expoConfig') {
      const config = JSON.parse(chunk.toString());
      delete config.updates?.requestHeaders;
      return JSON.stringify(config);
    }
    if (LINKED_MODULES.includes(source.id)) {
      const modules = JSON.parse(chunk.toString());
      delete modules['@sikemux/native'];
      return JSON.stringify(modules);
    }
    return chunk;
  },
};
