// Puts a signed app bundle on Google Play with the release notes for its version: a nightly on
// the closed testing track, a stable release on production, where it starts with a share of phones
// and widens once it holds up.
//
//   node scripts/play-release.mjs notes 0.1.0-nightly.5
//   node scripts/play-release.mjs upload 0.1.0-nightly.5 dist/Sikemux_0.1.0-nightly.5_android.aab
//   node scripts/play-release.mjs rollout 0.5        half of production phones
//   node scripts/play-release.mjs rollout 1          every phone
//
// `upload` signs in as the service account whose JSON key is in PLAY_SERVICE_ACCOUNT.
import { Buffer } from 'node:buffer';
import { createSign } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const PACKAGE = 'com.nodelike.sikemux.mobile';
const API = `https://androidpublisher.googleapis.com/androidpublisher/v3/applications/${PACKAGE}`;
const UPLOAD = `https://androidpublisher.googleapis.com/upload/androidpublisher/v3/applications/${PACKAGE}`;
const NOTES = resolve(dirname(fileURLToPath(import.meta.url)), '../../RELEASE_NOTES.md');
/** Google Play shows at most this many characters of "What's new", and refuses more. */
const NOTES_LIMIT = 500;

/** The notes under the file's `# <version>` heading, refused unless they are for this version. */
export function releaseNotes(version, text = readFileSync(NOTES, 'utf8')) {
  const [heading, ...rest] = text.trim().split('\n');
  if (heading.trim() !== `# ${version}`) {
    throw new Error(`mobile/RELEASE_NOTES.md is for "${heading.replace(/^#\s*/, '')}", not ${version}: write this release's notes first`);
  }
  const notes = rest.join('\n').trim();
  if (!notes) throw new Error('mobile/RELEASE_NOTES.md has no notes under its heading');
  if (/\]\(|\*\*|^#/m.test(notes))
    throw new Error('Google Play shows plain text: drop the Markdown links, bold and headings from mobile/RELEASE_NOTES.md');
  if (notes.length > NOTES_LIMIT)
    throw new Error(`mobile/RELEASE_NOTES.md is ${notes.length} characters; Google Play takes ${NOTES_LIMIT}`);
  return notes;
}

export function track(version) {
  return version.includes('-nightly.') ? 'alpha' : 'production';
}

/** The share of production phones a stable release reaches first. */
export const FIRST_ROLLOUT = 0.1;

/**
 * The release a new bundle makes on its track: testers get a nightly at once, production phones a stable one in stages.
 * Google Play stages only updates, so the app's first production release reaches every phone.
 */
export function trackRelease(version, versionCode, notes, firstOnProduction = false) {
  const release = { name: version, versionCodes: [String(versionCode)], releaseNotes: [{ language: 'en-US', text: notes }] };
  return track(version) === 'production' && !firstOnProduction
    ? { ...release, status: 'inProgress', userFraction: FIRST_ROLLOUT }
    : { ...release, status: 'completed' };
}

/** The production releases with the one rolling out widened to `fraction`, or finished at 1. */
export function widened(releases, fraction) {
  if (!(fraction > 0 && fraction <= 1)) throw new Error(`${fraction} is not a share of phones between 0 and 1`);
  const rolling = releases.find((release) => release.status === 'inProgress');
  if (!rolling) throw new Error('No production release is rolling out');
  if (fraction <= rolling.userFraction) throw new Error(`${rolling.name} already reaches ${rolling.userFraction} of phones`);
  const next = fraction === 1 ? { ...rolling, status: 'completed', userFraction: undefined } : { ...rolling, userFraction: fraction };
  return fraction === 1 ? [next] : releases.map((release) => (release === rolling ? next : release));
}

function base64url(value) {
  return Buffer.from(value).toString('base64url');
}

async function accessToken(account) {
  const now = Math.floor(Date.now() / 1000);
  const claims = {
    iss: account.client_email,
    scope: 'https://www.googleapis.com/auth/androidpublisher',
    aud: account.token_uri,
    iat: now,
    exp: now + 3600,
  };
  const unsigned = `${base64url(JSON.stringify({ alg: 'RS256', typ: 'JWT' }))}.${base64url(JSON.stringify(claims))}`;
  const signature = createSign('RSA-SHA256').update(unsigned).sign(account.private_key, 'base64url');
  const response = await fetch(account.token_uri, {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({ grant_type: 'urn:ietf:params:oauth:grant-type:jwt-bearer', assertion: `${unsigned}.${signature}` }),
  });
  const answer = await response.json();
  if (!response.ok) throw new Error(`Google refused the service account: ${answer.error_description ?? answer.error}`);
  return answer.access_token;
}

async function call(token, url, init = {}) {
  const response = await fetch(url, { ...init, headers: { authorization: `Bearer ${token}`, ...init.headers } });
  const answer = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(`Google Play: ${answer.error?.message ?? response.statusText} (${init.method ?? 'GET'} ${url})`);
  return answer;
}

async function upload(version, bundle) {
  const notes = releaseNotes(version);
  const name = track(version);
  const token = await accessToken(JSON.parse(process.env.PLAY_SERVICE_ACCOUNT ?? ''));
  const edit = await call(token, `${API}/edits`, { method: 'POST' });
  const uploaded = await call(token, `${UPLOAD}/edits/${edit.id}/bundles?uploadType=media`, {
    method: 'POST',
    headers: { 'content-type': 'application/octet-stream' },
    body: readFileSync(bundle),
  });
  const firstOnProduction =
    name === 'production' &&
    !(await call(token, `${API}/edits/${edit.id}/tracks/production`)).releases?.some((release) => release.status !== 'draft');
  const made = trackRelease(version, uploaded.versionCode, notes, firstOnProduction);
  await call(token, `${API}/edits/${edit.id}/tracks/${name}`, {
    method: 'PUT',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ track: name, releases: [made] }),
  });
  await call(token, `${API}/edits/${edit.id}:commit`, { method: 'POST' });
  const reach = made.status === 'inProgress' ? ` to ${FIRST_ROLLOUT * 100}% of phones; widen it with \`rollout\`` : '';
  console.log(`Google Play: ${version} (version code ${uploaded.versionCode}) is out on ${name}${reach}`);
}

async function rollout(fraction) {
  const token = await accessToken(JSON.parse(process.env.PLAY_SERVICE_ACCOUNT ?? ''));
  const edit = await call(token, `${API}/edits`, { method: 'POST' });
  const current = await call(token, `${API}/edits/${edit.id}/tracks/production`);
  const releases = widened(current.releases ?? [], fraction);
  await call(token, `${API}/edits/${edit.id}/tracks/production`, {
    method: 'PUT',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ track: 'production', releases }),
  });
  await call(token, `${API}/edits/${edit.id}:commit`, { method: 'POST' });
  console.log(`Google Play: production now reaches ${fraction * 100}% of phones`);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [command, version, bundle] = process.argv.slice(2);
  try {
    if (command === 'notes' && version) console.log(releaseNotes(version));
    else if (command === 'upload' && version && bundle) await upload(version, bundle);
    else if (command === 'rollout' && version) await rollout(Number(version));
    else {
      console.error('usage: play-release.mjs notes <version> | upload <version> <bundle.aab> | rollout <share of phones>');
      process.exit(2);
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  }
}
