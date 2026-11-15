import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { delimiter, join } from 'node:path';

import { native, PHONE_TARGETS, recordBuild, repo, rustupPath, toolchainChannel } from '../../scripts/native-build.mjs';

function fail(message) {
  console.error(`\n${message}\n`);
  process.exit(1);
}

function output(command, args, env) {
  const result = spawnSync(command, args, { cwd: repo, env, encoding: 'utf8' });
  return result.status === 0 ? result.stdout.trim() : null;
}

function ubrnVersion() {
  return JSON.parse(readFileSync(join(native, 'node_modules/uniffi-bindgen-react-native/package.json'), 'utf8')).version;
}

function checkRust(env, cargoBin, needed) {
  if (!existsSync(join(cargoBin, 'rustup'))) {
    fail(`rustup is not installed in ${cargoBin}. Install it from https://rustup.rs; Homebrew's Rust has no phone targets.`);
  }
  const channel = toolchainChannel();
  const version = output('rustc', ['-V'], env);
  if (!version?.startsWith(`rustc ${channel} `)) {
    fail(
      `rustc is ${version ?? 'missing'}, but rust-toolchain.toml asks for ${channel}.\n` +
        `Run \`rustup toolchain install\` in the repo, and make sure ${cargoBin} comes before Homebrew on PATH.`,
    );
  }
  const installed = (output('rustup', ['target', 'list', '--installed'], env) ?? '').split('\n');
  const missing = needed.filter((target) => !installed.includes(target));
  if (missing.length) fail(`The phone targets are not installed: run \`rustup target add ${missing.join(' ')}\`.`);
}

const args = process.argv.slice(2);
const { cargoBin, PATH } = rustupPath();
const env = { ...process.env, PATH: [join(repo, 'scripts/bin'), PATH].join(delimiter) };
const [command, platform] = args;
const help = args.includes('--help') || args.includes('-h');
const building = command === 'build' && platform in PHONE_TARGETS && !help;

if (building) {
  checkRust(env, cargoBin, PHONE_TARGETS[platform]);
  if (platform === 'android' && output('cargo', ['ndk', '--version'], env) === null) {
    fail('cargo-ndk is not installed: run `cargo install cargo-ndk`.');
  }
}

const flag = args.findIndex((arg) => arg === '--profile' || arg === '-p');
const release = args.includes('--release') || args.includes('-r');
const profile = flag >= 0 ? args[flag + 1] : release ? 'release' : 'debug';
const ubrn = [join(native, 'node_modules/.bin/ubrn'), ...args];
const ndk = env.ANDROID_NDK_HOME ?? env.ANDROID_NDK_ROOT ?? '';
const [program, ...programArgs] = building
  ? [
      'python3',
      join(repo, 'mobile/scripts/phone-core.py'),
      ...['--kind', `${platform}-${profile}`, '--extra', `ubrn ${ubrnVersion()} ndk ${ndk} ${args.join(' ')}`],
      ...['--out', 'mobile/native', '--stamp', `build/${platform}.key`],
      ...['--keep', 'android/CMakeLists.txt', 'android/build.gradle', 'android/cpp-adapter.cpp', 'android/src', 'cpp', 'src'],
      '--',
      ...ubrn,
    ]
  : ubrn;
const result = spawnSync(program, programArgs, { cwd: native, env, stdio: 'inherit' });
if (result.error) fail(`Could not run ubrn: ${result.error.message}. Run pnpm install in mobile/.`);
if (result.status !== 0) process.exit(result.status ?? 1);

if (building) recordBuild(platform, profile);
