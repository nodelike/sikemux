#!/usr/bin/env node
// Publishes the app's JavaScript and assets as a signed over-the-air update. Preparing and signing
// are separate steps so CI can keep the signing key away from the build and its dependencies.
import { Buffer } from 'node:buffer';
import { execFileSync, spawnSync } from 'node:child_process';
import { createHash, createPrivateKey, randomUUID, sign, verify, X509Certificate } from 'node:crypto';
import {
  appendFileSync,
  closeSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const app = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const certificate = join(app, 'certs/updates-certificate.pem');
const release = JSON.parse(readFileSync(join(app, '../../server/protocol/src/release.json'), 'utf8'));
const ASSET_URL = release.assetUrl;
const PLATFORMS = release.platforms;
const CHANNELS = release.channels;
const UUID = new RegExp(release.updateId);
const SHA256_HEX = /^[0-9a-f]{64}$/;
const CONTENT_TYPES = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  svg: 'image/svg+xml',
  ttf: 'font/ttf',
  otf: 'font/otf',
  woff: 'font/woff',
  woff2: 'font/woff2',
  json: 'application/json',
  xml: 'application/xml',
  mp3: 'audio/mpeg',
  wav: 'audio/wav',
  mp4: 'video/mp4',
};

/** Why the script stopped, for whoever ran it. */
export class Refused extends Error {}

function stop(message) {
  throw new Refused(message);
}

const sha256 = (bytes) => createHash('sha256').update(bytes);

function run(command, args, options = {}) {
  return execFileSync(command, args, { cwd: app, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024, ...options });
}

async function prepare(platform, channel, dir) {
  if (!PLATFORMS.includes(platform) || !CHANNELS.includes(channel)) usage();
  const { bindingsExist } = await import('../../scripts/native-build.mjs');
  if (!bindingsExist()) stop('The Rust client has not been built: run `pnpm native:android:release` in mobile/ first.');
  const { generate } = await import('./generate.mjs');
  await generate();

  const commit = run('git', ['rev-parse', 'HEAD']).trim();
  const env = { ...process.env, APP_VARIANT: 'production', SIKEMUX_MOBILE_CHANNEL: channel, EXPO_PUBLIC_COMMIT: commit.slice(0, 7) };
  const stdio = ['ignore', 'pipe', 'inherit'];
  const { runtimeVersion } = JSON.parse(run('npx', ['expo-updates', 'runtimeversion:resolve', '--platform', platform], { env, stdio }));
  if (typeof runtimeVersion !== 'string') stop('The app config has no runtime version, so an update could reach any build.');
  const reach = reaches(runtimeVersion, releasedRuntimes(platform));
  output('publish', String(reach !== 'none'));
  if (reach === 'unknown') console.warn(`::warning::No phone release says which runtime it has, so ${runtimeVersion} may reach no phone.`);
  if (reach === 'none') {
    console.warn(
      `::warning::No released ${platform} build has runtime ${runtimeVersion}: its native code changed since the last phone ` +
        'release, so this update would reach no phone. It is not published; the next phone release ships this code itself.',
    );
    return false;
  }

  // An update replaces the app's config as the app sees it, so it carries the production config along.
  const expoClient = JSON.parse(run('npx', ['expo', 'config', '--type', 'public', '--json'], { env, stdio }));
  delete expoClient.updates?.requestHeaders;

  const exported = mkdtempSync(join(tmpdir(), 'sikemux-export-'));
  run('npx', ['expo', 'export', '--platform', platform, '--output-dir', exported], { env, stdio: ['ignore', 'inherit', 'inherit'] });
  const files = JSON.parse(readFileSync(join(exported, 'metadata.json'), 'utf8')).fileMetadata[platform];

  mkdirSync(join(dir, 'assets'), { recursive: true });
  const store = (path, contentType, fileExtension) => {
    const bytes = readFileSync(join(exported, path));
    const hex = sha256(bytes).digest('hex');
    writeFileSync(join(dir, 'assets', hex), bytes);
    writeFileSync(join(dir, 'assets', `${hex}.type`), `${contentType}\n`);
    return {
      hash: sha256(bytes).digest('base64url'),
      // The same key Metro gives an asset inside the build, so the phone reuses the copy it shipped with.
      key: createHash('md5').update(bytes).digest('hex'),
      contentType,
      fileExtension,
      url: `${ASSET_URL}${hex}`,
    };
  };
  const launchAsset = store(files.bundle, 'application/javascript', '.bundle');
  const assets = [];
  const keys = new Set([launchAsset.key]);
  for (const { path, ext } of files.assets) {
    const asset = store(path, CONTENT_TYPES[ext] ?? 'application/octet-stream', `.${ext}`);
    if (!keys.has(asset.key)) assets.push(asset);
    keys.add(asset.key);
  }

  const message = run('git', ['log', '-1', '--format=%s', commit]).trim();
  const id = randomUUID();
  const createdAt = new Date().toISOString();
  const manifest = {
    id,
    createdAt,
    runtimeVersion,
    launchAsset,
    assets,
    metadata: {},
    extra: { expoClient, commit, message, channel },
  };
  writeFileSync(join(dir, 'manifest.json'), JSON.stringify(manifest));
  writeFileSync(
    join(dir, 'update.json'),
    `${JSON.stringify({ id, createdAt, platform, runtimeVersion, channel, commit, message }, null, 2)}\n`,
  );
  return true;
}

/** The asset each phone release carries with the runtime version of its build. */
export function runtimeAsset(platform) {
  return `runtime-${platform}.txt`;
}

/** The runtime versions the newest phone releases were built with, or null when GitHub can't be asked. */
export function releasedRuntimes(platform, gh = (args) => run('gh', args, { stdio: ['ignore', 'pipe', 'ignore'] })) {
  let tags;
  try {
    tags = JSON.parse(gh(['release', 'list', '--limit', '200', '--json', 'tagName']))
      .map((release) => release.tagName)
      .filter((tag) => tag.startsWith('mobile-v'))
      .slice(0, 20);
  } catch {
    return null;
  }
  const runtimes = [];
  for (const tag of tags) {
    try {
      runtimes.push(gh(['release', 'download', tag, '--pattern', runtimeAsset(platform), '--output', '-']).trim());
    } catch {
      continue;
    }
  }
  return runtimes;
}

/** Whether an update for `runtime` reaches a released build: `unknown` when no release says what it has. */
export function reaches(runtime, released) {
  if (!released || released.length === 0) return 'unknown';
  return released.includes(runtime) ? 'some' : 'none';
}

function output(name, value) {
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `${name}=${value}\n`);
}

/** Checks the update in `dir` is whole: every asset the manifest names is there and has the hash it claims. */
export function check(dir) {
  const update = JSON.parse(readFileSync(join(dir, 'update.json'), 'utf8'));
  const manifestBytes = readFileSync(join(dir, 'manifest.json'));
  const manifest = JSON.parse(manifestBytes.toString('utf8'));
  for (const field of ['id', 'createdAt', 'runtimeVersion']) {
    if (manifest[field] !== update[field]) stop(`update.json and manifest.json disagree on ${field}.`);
  }
  if (!UUID.test(update.id)) stop(`${update.id} is not an update id.`);
  if (!PLATFORMS.includes(update.platform) || !CHANNELS.includes(update.channel)) stop('update.json names an unknown platform or channel.');

  const named = new Set();
  let bytes = 0;
  for (const asset of [manifest.launchAsset, ...manifest.assets]) {
    const hex = asset.url.slice(ASSET_URL.length);
    if (!asset.url.startsWith(ASSET_URL) || !SHA256_HEX.test(hex)) stop(`${asset.url} is not an update asset address.`);
    const contents = readFileSync(join(dir, 'assets', hex));
    if (sha256(contents).digest('hex') !== hex || sha256(contents).digest('base64url') !== asset.hash) {
      stop(`The asset ${hex} does not match its hash.`);
    }
    const type = readFileSync(join(dir, 'assets', `${hex}.type`), 'utf8').trim();
    if (type !== asset.contentType) stop(`The asset ${hex} is stored as ${type} but the manifest says ${asset.contentType}.`);
    if (!named.has(hex)) bytes += contents.length;
    named.add(hex);
  }
  for (const file of readdirSync(join(dir, 'assets'))) {
    if (!named.has(file.replace(/\.type$/, ''))) stop(`assets/${file} is not named by the manifest.`);
  }
  return { update, manifestBytes, assets: named.size, bytes };
}

function signForPhones(bytes) {
  const pem = process.env.UPDATES_SIGNING_KEY;
  if (!pem) stop('UPDATES_SIGNING_KEY is not set: it holds the PEM private key updates are signed with.');
  const key = createPrivateKey(pem);
  const signature = sign('sha256', bytes, key);
  const cert = new X509Certificate(readFileSync(certificate));
  const now = Date.now();
  if (now < Date.parse(cert.validFrom) || now > Date.parse(cert.validTo)) stop('The update certificate has expired or is not valid yet.');
  if (!verify('sha256', bytes, cert.publicKey, signature)) {
    stop(`UPDATES_SIGNING_KEY is not the key of ${certificate}: phones would refuse what it signs.`);
  }
  return signature.toString('base64');
}

function summarise(lines) {
  console.log(lines.join('\n'));
  if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${lines.map((line) => `- ${line}`).join('\n')}\n`);
}

function ssh(command, input) {
  const host = process.env.DEPLOY_HOST;
  if (!host) stop('DEPLOY_HOST is not set: it names the server updates are sent to.');
  const key = process.env.DEPLOY_SSH_KEY_FILE;
  const args = [
    '-o',
    'StrictHostKeyChecking=yes',
    ...(key ? ['-i', key, '-o', 'IdentitiesOnly=yes'] : []),
    `sikemux-deploy@${host}`,
    command,
  ];
  const stdin = input === undefined ? 'ignore' : openSync(input, 'r');
  const result = spawnSync('ssh', args, { stdio: [stdin, 'inherit', 'inherit'] });
  if (typeof stdin === 'number') closeSync(stdin);
  if (result.status !== 0) stop(`citadel refused \`${command}\`.`);
}

function publish(dir, dryRun) {
  const { update, manifestBytes, assets, bytes } = check(dir);
  writeFileSync(join(dir, 'manifest.sig'), signForPhones(manifestBytes));

  const bundle = join(dir, '..', `sikemux-update-${update.id}.tar.gz`);
  // COPYFILE_DISABLE keeps macOS's tar from adding ._ files for extended attributes.
  run('tar', ['-czf', bundle, '-C', dir, 'update.json', 'manifest.json', 'manifest.sig', 'assets'], {
    env: { ...process.env, COPYFILE_DISABLE: '1' },
  });
  summarise([
    `Update ${update.id} for ${update.platform} on ${update.channel}, from ${update.commit.slice(0, 12)}`,
    `Runtime version ${update.runtimeVersion}`,
    `${assets} files, ${bytes} bytes; bundle ${bundle} (${statSync(bundle).size} bytes)`,
  ]);
  output('update_id', update.id);
  if (dryRun) {
    console.log('Dry run: not sent.');
    return;
  }
  ssh('publish-update', bundle);
  summarise([`Published ${update.id} to ${update.channel}`]);
}

/** Withdraws the update and sends a signed order for phones that took it to go back to the code in their build. */
function rollBack(id) {
  const directive = JSON.stringify({ type: 'rollBackToEmbedded', parameters: { commitTime: new Date().toISOString() } });
  const file = join(mkdtempSync(join(tmpdir(), 'sikemux-rollback-')), 'directive.json');
  writeFileSync(file, JSON.stringify({ directive, signature: signForPhones(Buffer.from(directive)) }));
  ssh(`roll-back-update ${id}`, file);
  summarise([`Rolled phones back from ${id} to the code in their build`]);
}

function usage() {
  stop(
    'usage: publish-update.mjs prepare <android|ios> <nightly|stable> <dir>\n' +
      '       publish-update.mjs publish <dir> [--dry-run]\n' +
      '       publish-update.mjs <android|ios> <nightly|stable> [--dry-run]\n' +
      '       publish-update.mjs promote <update id>\n' +
      '       publish-update.mjs withdraw <update id>\n' +
      '       publish-update.mjs roll-back <update id>',
  );
}

async function main(argv) {
  const dryRun = argv.includes('--dry-run');
  const [command, ...args] = argv.filter((arg) => arg !== '--dry-run');
  if (command === 'prepare' && args.length === 3) {
    const dir = resolve(args[2]);
    if (existsSync(dir) && readdirSync(dir).length > 0) stop(`${dir} is not empty.`);
    await prepare(args[0], args[1], dir);
  } else if (command === 'publish' && args.length === 1) {
    publish(resolve(args[0]), dryRun);
  } else if (command === 'promote' && args.length === 1) {
    if (!UUID.test(args[0])) stop(`${args[0]} is not an update id.`);
    ssh(`promote-update ${args[0]}`);
    summarise([`Promoted ${args[0]} to stable`]);
  } else if (command === 'withdraw' && args.length === 1) {
    if (!UUID.test(args[0])) stop(`${args[0]} is not an update id.`);
    ssh(`withdraw-update ${args[0]}`);
    summarise([`Withdrew ${args[0]}`]);
  } else if (command === 'roll-back' && args.length === 1) {
    if (!UUID.test(args[0])) stop(`${args[0]} is not an update id.`);
    rollBack(args[0]);
  } else if (PLATFORMS.includes(command) && args.length === 1) {
    const dir = join(mkdtempSync(join(tmpdir(), 'sikemux-update-')), 'update');
    if (await prepare(command, args[0], dir)) publish(dir, dryRun);
  } else {
    usage();
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  try {
    await main(process.argv.slice(2));
  } catch (error) {
    if (!(error instanceof Refused)) throw error;
    console.error(`\n${error.message}\n`);
    process.exit(1);
  }
}
