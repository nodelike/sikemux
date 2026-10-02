#!/usr/bin/env node
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { bindingsExist, buildCommand, libraries, readBuild, releaseCommand } from '../../scripts/native-build.mjs';
import { generate } from './generate.mjs';

const app = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const PLATFORMS = ['ios', 'android'];
const VARIANTS = ['development', 'production'];

const argv = process.argv.slice(2);
const prebuildOnly = argv[0] === 'prebuild';
const [platform, variant = prebuildOnly ? 'development' : undefined, ...rest] = prebuildOnly ? argv.slice(1) : argv;
if (!PLATFORMS.includes(platform) || !VARIANTS.includes(variant)) {
  console.error(
    'usage: run.mjs <ios|android> <development|production> [expo run args...]\n' +
      '       run.mjs prebuild <ios|android> [development|production]',
  );
  process.exit(1);
}

function stop(message) {
  console.error(`\n${message}\n`);
  process.exit(1);
}

if (!bindingsExist() || libraries(platform).length === 0) {
  stop(`The Rust client has not been built for ${platform}: run \`${buildCommand[platform]}\` in mobile/ first.`);
}
if (platform === 'ios' && rest.includes('--device') && readBuild('ios')?.simOnly) {
  stop('The Rust client was built for the simulator only: run `pnpm native:ios` in mobile/ to add the device slice.');
}
if (variant === 'production') {
  const profile = readBuild(platform)?.profile;
  if (profile !== 'mobile') {
    stop(
      `A release build needs the Rust client built with the small \`mobile\` profile, but the one on disk ` +
        `${profile ? `was built with \`${profile}\`` : 'was not built by pnpm native:*'}: run \`${releaseCommand[platform]}\` in mobile/ first.`,
    );
  }
}

await generate();

const env = { ...process.env, APP_VARIANT: variant };
// A clean prebuild deletes android/local.properties, which is where Gradle found the SDK.
const androidStudioSdk = join(homedir(), 'Library/Android/sdk');
if (!env.ANDROID_HOME && existsSync(androidStudioSdk)) env.ANDROID_HOME = androidStudioSdk;
const expo = (...args) => execFileSync('npx', ['expo', ...args], { cwd: app, stdio: 'inherit', env });

function configHash() {
  const hash = createHash('sha256').update(variant);
  const plugins = readdirSync(join(app, 'plugins')).sort();
  const files = ['app.json', 'app.config.js', ...plugins.map((name) => join('plugins', name))];
  for (const file of files) hash.update(file).update(readFileSync(join(app, file)));
  const { dependencies } = JSON.parse(readFileSync(join(app, 'package.json'), 'utf8'));
  return hash.update(JSON.stringify(dependencies)).digest('hex');
}

// The native project is generated from the app's config for one variant, so a change to either regenerates it.
const marker = join(app, platform, '.sikemux-prebuild');
const wanted = configHash();
const built = existsSync(marker) ? readFileSync(marker, 'utf8').trim() : null;
if (prebuildOnly || built !== wanted) {
  expo('prebuild', '--clean', '--platform', platform);
  writeFileSync(marker, `${wanted}\n`);
}
if (!prebuildOnly) expo(`run:${platform}`, ...rest);
