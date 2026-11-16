// `internal` waits for App Store Connect to finish processing an uploaded iPhone build and sets its
// "What to Test" from the release notes; internal testers get it then, with no review.
//
//   node scripts/testflight-release.mjs internal 0.1.0-nightly.10 100010
//
// `submit` does the same and also hands the build to the external Testers group through beta review.
// Apple only lets external testers install a build it has reviewed.
//
// `app-store` puts a build up for App Store review as a stable version, released by itself once Apple
// approves. A stable release promotes the build a nightly already sent to TestFlight:
//
//   node scripts/testflight-release.mjs app-store 0.2.0 200007
//
// `prune-certificates` revokes the development certificates earlier CI archives made. Each runner
// starts with an empty keychain, so Xcode makes a new one every run until Apple's limit refuses.
//
// Signs in with the App Store Connect API key in APP_STORE_CONNECT_KEY, _KEY_ID and _ISSUER_ID.
import { Buffer } from 'node:buffer';
import { createSign } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { setTimeout as sleep } from 'node:timers/promises';

import { releaseNotes } from '../../app/scripts/play-release.mjs';

/** The App Store build's bundle id, as project.yml's Release settings name it. */
const BUNDLE_ID = /\n {4}Release:\n(?: {6}.*\n)*? {6}SIKEMUX_BUNDLE_ID: (\S+)\n/.exec(
  readFileSync(new URL('../project.yml', import.meta.url), 'utf8'),
)?.[1];
if (!BUNDLE_ID) throw new Error('project.yml names no SIKEMUX_BUNDLE_ID for Release');
const API = 'https://api.appstoreconnect.apple.com';
export const TESTERS_GROUP = 'Testers';
const PROCESSING_LIMIT_MS = 40 * 60 * 1000;
const POLL_MS = 30 * 1000;

function nextStep(externalState) {
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
    const processing = candidate?.attributes.processingState;
    const detail = found.included?.find((item) => item.type === 'buildBetaDetails');
    // A build reads VALID a while before TestFlight knows it, and adding it to a group then fails.
    const state = processing === 'VALID' && !detail ? 'VALID but not in TestFlight yet' : processing;
    if (state === 'VALID') return { id: candidate.id, external: detail.attributes.externalBuildState };
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

async function internal(version, build) {
  const apps = await call(`/v1/apps?filter[bundleId]=${BUNDLE_ID}`);
  const appId = apps.data[0]?.id;
  if (!appId) throw new Error(`App Store Connect has no app with the bundle id ${BUNDLE_ID}`);
  const { id: buildId } = await processedBuild(appId, version.replace(/-.*$/, ''), build);
  await setWhatToTest(buildId, releaseNotes(version));
  console.log(`TestFlight: ${version} (${build}) is out to internal testers`);
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

const IN_REVIEW = ['WAITING_FOR_REVIEW', 'IN_REVIEW', 'PENDING_DEVELOPER_RELEASE', 'PROCESSING_FOR_APP_STORE', 'READY_FOR_SALE'];
const RELEASED = ['READY_FOR_SALE', 'REPLACED_WITH_NEW_VERSION'];
const EDITABLE = ['PREPARE_FOR_SUBMISSION', 'DEVELOPER_REJECTED', 'REJECTED', 'METADATA_REJECTED'];

async function appStoreVersion(appId, version) {
  const found = await call(`/v1/apps/${appId}/appStoreVersions?filter[platform]=IOS&limit=200`);
  const released = found.data.some((item) => RELEASED.includes(item.attributes.appStoreState));
  // Apple keeps one version open for editing, so an unsubmitted older one is renamed to this one.
  const existing =
    found.data.find((item) => item.attributes.versionString === version) ??
    found.data.find((item) => EDITABLE.includes(item.attributes.appStoreState));
  if (existing) {
    await call(`/v1/appStoreVersions/${existing.id}`, {
      method: 'PATCH',
      body: JSON.stringify({
        data: { type: 'appStoreVersions', id: existing.id, attributes: { versionString: version, releaseType: 'AFTER_APPROVAL' } },
      }),
    });
    return { id: existing.id, state: existing.attributes.appStoreState, released };
  }
  const made = await call('/v1/appStoreVersions', {
    method: 'POST',
    body: JSON.stringify({
      data: {
        type: 'appStoreVersions',
        attributes: { platform: 'IOS', versionString: version, releaseType: 'AFTER_APPROVAL' },
        relationships: { app: { data: { type: 'apps', id: appId } } },
      },
    }),
  });
  return { id: made.data.id, state: made.data.attributes.appStoreState, released };
}

/** Apple takes "What's New" only on an update, not on an app's first version. */
async function setWhatsNew(versionId, notes) {
  const found = await call(`/v1/appStoreVersions/${versionId}/appStoreVersionLocalizations`);
  const english = found.data.find((item) => item.attributes.locale === 'en-US');
  if (!english) throw new Error('The App Store version has no en-US localization');
  await call(`/v1/appStoreVersionLocalizations/${english.id}`, {
    method: 'PATCH',
    body: JSON.stringify({ data: { type: 'appStoreVersionLocalizations', id: english.id, attributes: { whatsNew: notes } } }),
  });
}

async function reviewSubmission(appId) {
  const open = await call(`/v1/reviewSubmissions?filter[app]=${appId}&filter[platform]=IOS&filter[state]=READY_FOR_REVIEW`);
  if (open.data[0]) return open.data[0].id;
  const made = await call('/v1/reviewSubmissions', {
    method: 'POST',
    body: JSON.stringify({
      data: { type: 'reviewSubmissions', attributes: { platform: 'IOS' }, relationships: { app: { data: { type: 'apps', id: appId } } } },
    }),
  });
  return made.data.id;
}

async function appStore(version, build) {
  const apps = await call(`/v1/apps?filter[bundleId]=${BUNDLE_ID}`);
  const appId = apps.data[0]?.id;
  if (!appId) throw new Error(`App Store Connect has no app with the bundle id ${BUNDLE_ID}`);
  const { id: buildId } = await processedBuild(appId, version, build);

  const target = await appStoreVersion(appId, version);
  if (IN_REVIEW.includes(target.state)) {
    console.log(`App Store: ${version} is already ${target.state}`);
    return;
  }
  await call(`/v1/appStoreVersions/${target.id}/relationships/build`, {
    method: 'PATCH',
    body: JSON.stringify({ data: { type: 'builds', id: buildId } }),
  });
  if (target.released) await setWhatsNew(target.id, releaseNotes(version));

  const submission = await reviewSubmission(appId);
  await call('/v1/reviewSubmissionItems', {
    method: 'POST',
    body: JSON.stringify({
      data: {
        type: 'reviewSubmissionItems',
        relationships: {
          reviewSubmission: { data: { type: 'reviewSubmissions', id: submission } },
          appStoreVersion: { data: { type: 'appStoreVersions', id: target.id } },
        },
      },
    }),
  });
  await call(`/v1/reviewSubmissions/${submission}`, {
    method: 'PATCH',
    body: JSON.stringify({ data: { type: 'reviewSubmissions', id: submission, attributes: { submitted: true } } }),
  });
  console.log(`App Store: ${version} (${build}) is submitted for review and releases once Apple approves it`);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [command, version, build] = process.argv.slice(2);
  try {
    if (command === 'internal' && version && build) await internal(version, build);
    else if (command === 'submit' && version && build) await submit(version, build);
    else if (command === 'app-store' && version && build) await appStore(version, build);
    else if (command === 'prune-certificates') await pruneCertificates();
    else {
      console.error('usage: testflight-release.mjs internal|submit <version> <build number> | app-store <version> <build number> | prune-certificates');
      process.exit(2);
    }
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
}
