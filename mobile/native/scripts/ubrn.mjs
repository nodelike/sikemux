import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';

import { native, PHONE_TARGETS, recordBuild, repo, rustupPath, toolchainChannel } from '../../scripts/native-build.mjs';

function fail(message) {
  console.error(`\n${message}\n`);
  process.exit(1);
}

function output(command, args, env) {
  const result = spawnSync(command, args, { cwd: repo, env, encoding: 'utf8' });
  return result.status === 0 ? result.stdout.trim() : null;
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
const env = { ...process.env, PATH };
const [command, platform] = args;
const help = args.includes('--help') || args.includes('-h');
const building = command === 'build' && platform in PHONE_TARGETS && !help;
const simOnly = args.includes('--sim-only');

if (building) {
  checkRust(env, cargoBin, simOnly ? ['aarch64-apple-ios-sim'] : PHONE_TARGETS[platform]);
  if (platform === 'android' && output('cargo', ['ndk', '--version'], env) === null) {
    fail('cargo-ndk is not installed: run `cargo install cargo-ndk`.');
  }
}

const result = spawnSync(join(native, 'node_modules/.bin/ubrn'), args, { cwd: native, env, stdio: 'inherit' });
if (result.error) fail(`Could not run ubrn: ${result.error.message}. Run pnpm install in mobile/.`);
if (result.status !== 0) process.exit(result.status ?? 1);

if (building) {
  const flag = args.findIndex((arg) => arg === '--profile' || arg === '-p');
  const release = args.includes('--release') || args.includes('-r');
  const profile = flag >= 0 ? args[flag + 1] : release ? 'release' : 'debug';
  recordBuild(platform, profile, simOnly);
}
