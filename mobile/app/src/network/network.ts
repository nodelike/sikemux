import { File, Paths } from 'expo-file-system';
import type { MinimumVersions, Network, Relay } from '@sikemux/protocol';
import { connectApi } from '@shared/account/api';
import type { RelaySetting } from '@sikemux/native';

import { externalValue, useExternalValue } from '@shared/lib/externalValue';

import { OFFLINE } from '@/account/api';
import { apiUrl } from '@/account/config';
import { translate } from '@/i18n';
import { onNetworkChange } from './connectivity';
import { installedRelease } from './installed';
import { tooOld } from './versions';

/** Where hosts listen when nothing better is known. Never iroh's public relays: no host listens there. */
export const DEFAULT_RELAYS: Relay[] = [{ url: 'https://relay.sikemux.com/', region: 'default', quicPort: 7842 }];

const FETCH_TIMEOUT_MS = 3000;
/** The server lets its answer be cached for five minutes, so asking sooner learns nothing. */
const FRESH_FOR_MS = 5 * 60_000;
/** After the server could not be reached, each new connection does not wait on it again for this long. */
const UNREACHED_FOR_MS = 30_000;

/** The last answer, for when the server is out of reach. Nothing in it is secret. */
const saved = new File(Paths.document, 'network.json');

export type UpdateRequired = { current: string; minimum: string };

const required = externalValue<UpdateRequired | null>(null);
let reading: { until: number; fresh: boolean; relays: Promise<Relay[]> } | undefined;

function usableRelays(value: unknown): Relay[] {
  if (!Array.isArray(value)) return [];
  return value.filter(
    (relay): relay is Relay =>
      typeof relay?.url === 'string' && /^https?:\/\//.test(relay.url) && (relay.quicPort === null || Number.isInteger(relay.quicPort)),
  );
}

function minimumVersions(value: unknown): MinimumVersions | null {
  const minimum = value as MinimumVersions | undefined;
  const fine = (platform: keyof MinimumVersions) =>
    typeof minimum?.[platform]?.nightly === 'string' && typeof minimum[platform].stable === 'string';
  return fine('ios') && fine('android') && fine('macos') ? (minimum as MinimumVersions) : null;
}

/** The server's answer, or null when it could not be reached in time; the saved copy stands in then. */
async function fetchNetwork(): Promise<Network | null> {
  return connectApi({ url: apiUrl(), offline: translate(OFFLINE), timeoutMs: FETCH_TIMEOUT_MS, say: translate })
    .getNetwork()
    .catch(() => null);
}

async function savedRelays(): Promise<Relay[]> {
  try {
    return saved.exists ? usableRelays((JSON.parse(await saved.text()) as { relays?: unknown }).relays) : [];
  } catch {
    return [];
  }
}

function setRequired(next: UpdateRequired | null) {
  const now = required.get();
  if (next?.current === now?.current && next?.minimum === now?.minimum) return;
  required.set(next);
}

/** Only an answer from the server decides this app is too old; failing to reach it never does. */
function checkMinimum(minimum: MinimumVersions) {
  const release = installedRelease();
  if (!release) return;
  const oldest = tooOld(release.version, minimum[release.platform]);
  setRequired(oldest ? { current: release.version, minimum: oldest } : null);
}

async function readRelays(): Promise<{ relays: Relay[]; fresh: boolean }> {
  const answer = await fetchNetwork();
  const minimum = minimumVersions(answer?.minimumVersions);
  if (minimum) checkMinimum(minimum);
  const relays = usableRelays(answer?.relays);
  if (relays.length > 0) {
    try {
      saved.write(JSON.stringify(answer));
    } catch (error) {
      console.warn('sikemux: could not keep the network answer on this phone', error);
    }
    return { relays, fresh: true };
  }
  const copy = await savedRelays();
  return { relays: copy.length > 0 ? copy : DEFAULT_RELAYS, fresh: false };
}

/**
 * The relays hosts listen on, best first: the server's answer, else the last one saved, else the built-in relay.
 * Relays rarely change, so a saved copy is used at once while the server is asked behind it; only a phone that
 * has never heard the answer waits for it.
 */
export function currentRelays(): Promise<Relay[]> {
  if (reading && Date.now() < reading.until) return reading.relays;
  const read = readRelays();
  const relays = savedRelays().then((copy) => (copy.length > 0 ? copy : read.then((result) => result.relays)));
  const current = { until: Date.now() + FRESH_FOR_MS, fresh: true, relays };
  reading = current;
  read.then(
    (result) => {
      if (result.fresh || reading !== current) return;
      current.fresh = false;
      current.until = Date.now() + UNREACHED_FOR_MS;
    },
    () => {
      if (reading === current) reading = undefined;
    },
  );
  return relays;
}

/** Asks the server again on the next connection, unless its last answer came through. */
export function askForRelaysAgain() {
  if (reading && !reading.fresh) reading = undefined;
}

onNetworkChange((change) => {
  if (change === 'regained') askForRelaysAgain();
});

export function relaySettings(relays: Relay[]): RelaySetting[] {
  return relays.map((relay) => ({ url: relay.url, quicPort: relay.quicPort ?? undefined }));
}

export function updateRequired(): UpdateRequired | null {
  return required.get();
}

export function useUpdateRequired(): UpdateRequired | null {
  return useExternalValue(required);
}
