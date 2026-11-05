import { File, Paths } from 'expo-file-system';
import type { Device } from '@sikemux/protocol';

import { readWhole, writeWhole } from './disk';

/** The hosts an account had when this phone last looked, so only one that signs in afterwards connects by itself. */
export type SeenHosts = { account: string; keys: string[] };

/**
 * Which of `hosts` are new on `account` since `seen`. The first look at an account only remembers what is there:
 * hosts it already had wait in Devices for a tap. A host is new once, so one that turned the phone down never asks again.
 */
export function arrivals(seen: SeenHosts | undefined, account: string, hosts: Device[]): { seen: SeenHosts; arrived: Device[] } {
  if (seen?.account !== account) return { seen: { account, keys: hosts.map((host) => host.key) }, arrived: [] };
  const known = new Set(seen.keys);
  const arrived = hosts.filter((host) => !known.has(host.key));
  return { seen: { account, keys: [...seen.keys, ...arrived.map((host) => host.key)] }, arrived };
}

const store = new File(Paths.document, 'seen-hosts.json');

function isSeen(value: unknown): value is SeenHosts {
  const seen = value as SeenHosts | null;
  return typeof seen?.account === 'string' && Array.isArray(seen.keys) && seen.keys.every((key) => typeof key === 'string');
}
let looking: Promise<unknown> = Promise.resolve();

/** The hosts that joined the account since the phone last looked, remembering them as seen. */
export function hostsArrived(account: string, hosts: Device[]): Promise<Device[]> {
  const next = looking.then(async () => {
    // A list that cannot be read counts as a first look, so hosts already there wait in Devices rather than all connecting.
    const seen = (await readWhole(store, isSeen)).value;
    const found = arrivals(seen, account, hosts);
    if (found.arrived.length || seen?.account !== account) writeWhole(store, JSON.stringify(found.seen));
    return found.arrived;
  });
  looking = next.catch(() => {});
  return next;
}
