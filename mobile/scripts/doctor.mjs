import { spawnSync } from 'node:child_process';
import { existsSync, readdirSync } from 'node:fs';
import { homedir } from 'node:os';
import { delimiter, join } from 'node:path';

import { bindingsExist, buildCommand, libraries, PHONE_TARGETS, readBuild, repo, rustupPath, toolchainChannel } from './native-build.mjs';

let failed = false;
const ok = (message) => console.log(`  ok    ${message}`);
const warn = (message) => console.log(`  warn  ${message}`);
const fail = (message) => {
  failed = true;
  console.log(`  FAIL  ${message}`);
};

function run(command, args, env = process.env) {
  const result = spawnSync(command, args, { cwd: repo, env, encoding: 'utf8' });
  return result.status === 0 ? `${result.stdout}${result.stderr}`.trim() : null;
}

function firstOnPath(name) {
  for (const dir of (process.env.PATH ?? '').split(delimiter)) {
    if (dir && existsSync(join(dir, name))) return join(dir, name);
  }
  return null;
}

console.log('Rust');
const { cargoBin, PATH } = rustupPath();
const rustupEnv = { ...process.env, PATH };
const cargo = firstOnPath('cargo');
if (!existsSync(join(cargoBin, 'rustup'))) fail(`rustup is not installed in ${cargoBin}: see https://rustup.rs`);
else if (cargo === join(cargoBin, 'cargo')) ok(`cargo on PATH is rustup's (${cargo})`);
else warn(`cargo on PATH is ${cargo ?? 'missing'}, not rustup's; the native:* scripts put ${cargoBin} first themselves`);

const channel = toolchainChannel();
const version = run('rustc', ['-V'], rustupEnv);
if (version?.startsWith(`rustc ${channel} `)) ok(version);
else fail(`rustc is ${version ?? 'missing'}, rust-toolchain.toml asks for ${channel}: run \`rustup toolchain install\``);

const installed = (run('rustup', ['target', 'list', '--installed'], rustupEnv) ?? '').split('\n');
const missingTargets = Object.values(PHONE_TARGETS)
  .flat()
  .filter((target) => !installed.includes(target));
if (missingTargets.length) fail(`missing targets: run \`rustup target add ${missingTargets.join(' ')}\``);
else ok(`targets: ${Object.values(PHONE_TARGETS).flat().join(', ')}`);

const ndkTool = run('cargo', ['ndk', '--version'], rustupEnv);
if (ndkTool) ok(ndkTool);
else warn('cargo-ndk is missing (Android only): run `cargo install cargo-ndk`');

console.log('\niOS');
const xcode = run('xcodebuild', ['-version']);
if (xcode) ok(xcode.split('\n')[0]);
else fail('Xcode is missing, or xcode-select points at the Command Line Tools: run `sudo xcode-select -s /Applications/Xcode.app`');
if (run('xcrun', ['simctl', 'help']) !== null) ok('xcrun simctl');
else fail('xcrun simctl does not run');

console.log('\nAndroid');
const sdk = process.env.ANDROID_HOME ?? [join(homedir(), 'Library/Android/sdk')].find(existsSync);
if (!sdk || !existsSync(sdk)) {
  warn('no Android SDK: set ANDROID_HOME (Android only)');
} else {
  ok(`SDK at ${sdk}${process.env.ANDROID_HOME ? '' : ' (ANDROID_HOME is not set; the run scripts find it here)'}`);
  const ndkHome = process.env.ANDROID_NDK_HOME;
  const ndks = existsSync(join(sdk, 'ndk')) ? readdirSync(join(sdk, 'ndk')) : [];
  if (ndkHome && existsSync(ndkHome)) ok(`NDK at ${ndkHome}`);
  else if (ndks.length) ok(`NDK ${ndks.sort().at(-1)}`);
  else warn('no NDK: install one from Android Studio, SDK Manager, SDK Tools');
}

console.log('\nBuilt');
if (bindingsExist()) ok('native bindings in native/src');
else warn(`no native bindings: run \`${buildCommand.ios}\``);
for (const platform of ['ios', 'android']) {
  const built = libraries(platform);
  const record = readBuild(platform);
  if (!built.length) {
    warn(`no ${platform} library: run \`${buildCommand[platform]}\``);
    continue;
  }
  const how = record ? `${record.profile} profile${record.simOnly ? ', simulator only' : ''}` : 'built outside pnpm native:*';
  ok(`${platform} library, ${built.length} slice${built.length > 1 ? 's' : ''} (${how})`);
}

process.exit(failed ? 1 : 0);
