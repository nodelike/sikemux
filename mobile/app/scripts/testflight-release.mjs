// Hands an uploaded iPhone build to the external testers: waits for App Store Connect to finish
// processing it, sets "What to Test" from the release notes, adds it to the Testers group and
// submits it for beta review. Apple only lets external testers install a build it has reviewed.
//
//   node scripts/testflight-release.mjs submit 0.1.0-nightly.10 10010
//
// `prune-certificates` revokes the development certificates earlier CI archives made. Each runner
// starts with an empty keychain, so Xcode makes a new one every run until Apple's limit refuses.
//
// Signs in with the App Store Connect API key in APP_STORE_CONNECT_KEY, _KEY_ID and _ISSUER_ID.
import { Buffer } from 'node:buffer';
import { createSign } from 'node:crypto';
import { setTimeout as sleep } from 'node:timers/promises';

import { releaseNotes } from './play-release.mjs';

const BUNDLE_ID = 'com.nodelike.sikemux.mobile';
const API = 'https://api.appstoreconnect.apple.com';
export const TESTERS_GROUP = 'Testers';
const PROCESSING_LIMIT_MS = 40 * 60 * 1000;
const POLL_MS = 30 * 1000;

/** What a build's external TestFlight state asks of us next. */
export function nextStep(externalState) {
  switch (externalState) {
    case 'PROCESSING':
      return 'wait';
    case 'READY_FOR_BETA_SUBMISSION':
      return 'submit';
    case 'WAITING_FOR_BETA_REVIEW':
    case 'IN_BETA_REVIEW':
    case 'BETA_APPROVED':
    case 'IN_BETA_TESTING':
      return 'done';
    default:
      throw new Error(`TestFlight cannot send this build to testers: its external state is ${externalState}`);
  }
}

/** The App Store Connect token; Apple refuses one that lives longer than 20 minutes. */
function token() {
  const now = Math.floor(Date.now() / 1000);
  const encode = (value) => Buffer.from(JSON.stringify(value)).toString('base64url');
  const unsigned = `${encode({ alg: 'ES256', kid: process.env.APP_STORE_CONNECT_KEY_ID, typ: 'JWT' })}.${encode({
    iss: process.env.APP_STORE_CONNECT_ISSUER_ID,
    iat: now,
    exp: now + 15 * 60,
    aud: 'appstoreconnect-v1',
  })}`;
  const signature = createSign('SHA256')
    .update(unsigned)
    .sign({ key: process.env.APP_STORE_CONNECT_KEY ?? '', dsaEncoding: 'ieee-p1363' }, 'base64url');
  return `${unsigned}.${signature}`;
}

const TRANSIENT_RETRIES = 4;

async function call(path, init = {}) {
  let response;
  for (let attempt = 0; ; attempt++) {
    response = await fetch(`${API}${path}`, {
      ...init,
      headers: { authorization: `Bearer ${token()}`, 'content-type': 'application/json', ...init.headers },
    });
    const transient = response.status === 429 || response.status >= 500;
    if (!transient || attempt === TRANSIENT_RETRIES) break;
    console.log(`App Store Connect answered ${response.status}; asking again in ${10 * (attempt + 1)} seconds`);
    await sleep(10_000 * (attempt + 1));
  }
  const answer = response.status === 204 ? {} : await response.json().catch(() => ({}));
  if (!response.ok) {
    const details = answer.errors?.map((error) => error.detail ?? error.title).join('; ') ?? response.statusText;
    throw new Error(`App Store Connect: ${details} (${init.method ?? 'GET'} ${path})`);
  }
  return answer;
}

async function processedBuild(appId, base, build) {
  const query = `filter[app]=${appId}&filter[version]=${build}&filter[preReleaseVersion.version]=${base}&include=buildBetaDetail`;
  const deadline = Date.now() + PROCESSING_LIMIT_MS;
  for (;;) {
    const found = await call(`/v1/builds?${query}`);
    const candidate = found.data[0];
    const state = candidate?.attributes.processingState;
    if (state === 'VALID') {
      const detail = found.included?.find((item) => item.type === 'buildBetaDetails');
      return { id: candidate.id, external: detail?.attributes.externalBuildState };
    }
    if (state === 'FAILED' || state === 'INVALID') throw new Error(`App Store Connect could not process build ${build}: ${state}`);
    if (Date.now() > deadline) throw new Error(`Build ${build} was still ${state ?? 'not listed'} after 40 minutes`);
    console.log(`TestFlight: build ${build} is ${state ?? 'not listed yet'}; checking again in 30 seconds`);
    await sleep(POLL_MS);
  }
}

async function setWhatToTest(buildId, notes) {
  const existing = await call(`/v1/builds/${buildId}/betaBuildLocalizations`);
  const english = existing.data.find((item) => item.attributes.locale === 'en-US');
  if (english) {
    await call(`/v1/betaBuildLocalizations/${english.id}`, {
      method: 'PATCH',
      body: JSON.stringify({ data: { type: 'betaBuildLocalizations', id: english.id, attributes: { whatsNew: notes } } }),
    });
    return;
  }
  await call('/v1/betaBuildLocalizations', {
    method: 'POST',
    body: JSON.stringify({
      data: {
        type: 'betaBuildLocalizations',
        attributes: { locale: 'en-US', whatsNew: notes },
        relationships: { build: { data: { type: 'builds', id: buildId } } },
      },
    }),
  });
}

const DEVELOPMENT_TYPES = ['DEVELOPMENT', 'IOS_DEVELOPMENT'];

/** A certificate Xcode made through the API key, whose private key went with its runner. */
export function madeByCi(certificate) {
  const { certificateType, name = '', displayName = '' } = certificate.attributes;
  return DEVELOPMENT_TYPES.includes(certificateType) && `${name} ${displayName}`.includes('Created via API');
}

async function pruneCertificates() {
  const found = await call(`/v1/certificates?filter[certificateType]=${DEVELOPMENT_TYPES.join(',')}&limit=200`);
  const stale = found.data.filter(madeByCi);
  for (const certificate of found.data) {
    const { name, displayName, expirationDate } = certificate.attributes;
    console.log(`TestFlight: ${stale.includes(certificate) ? 'revoking' : 'keeping'} ${name} (${displayName}), expires ${expirationDate}`);
    if (stale.includes(certificate)) await call(`/v1/certificates/${certificate.id}`, { method: 'DELETE' });
  }
  console.log(`TestFlight: revoked ${stale.length} development certificates earlier CI runs made`);
}

async function submit(version, build) {
  const notes = releaseNotes(version);
  const base = version.replace(/-.*$/, '');
  const apps = await call(`/v1/apps?filter[bundleId]=${BUNDLE_ID}`);
  const appId = apps.data[0]?.id;
  if (!appId) throw new Error(`App Store Connect has no app with the bundle id ${BUNDLE_ID}`);

  const { id: buildId, external } = await processedBuild(appId, base, build);
  await setWhatToTest(buildId, notes);

  const groups = await call(`/v1/apps/${appId}/betaGroups?limit=200`);
  const group = groups.data.find((item) => item.attributes.name === TESTERS_GROUP && !item.attributes.isInternalGroup);
  if (!group) throw new Error(`App Store Connect has no external TestFlight group named ${TESTERS_GROUP}`);
  await call(`/v1/betaGroups/${group.id}/relationships/builds`, {
    method: 'POST',
    body: JSON.stringify({ data: [{ type: 'builds', id: buildId }] }),
  });

  if (nextStep(external) === 'submit') {
    await call('/v1/betaAppReviewSubmissions', {
      method: 'POST',
      body: JSON.stringify({
        data: { type: 'betaAppReviewSubmissions', relationships: { build: { data: { type: 'builds', id: buildId } } } },
      }),
    });
    console.log(`TestFlight: ${version} (${build}) is in the ${TESTERS_GROUP} group and submitted for beta review`);
  } else {
    console.log(`TestFlight: ${version} (${build}) is in the ${TESTERS_GROUP} group; its beta review state is ${external}`);
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [command, version, build] = process.argv.slice(2);
  try {
    if (command === 'submit' && version && build) await submit(version, build);
    else if (command === 'prune-certificates') await pruneCertificates();
    else {
      console.error('usage: testflight-release.mjs submit <version> <build number> | prune-certificates');
      process.exit(2);
    }
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
}
