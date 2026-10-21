import { formatAgo } from '@mac/lib/format';
import type { Snapshot } from '@/core/protocol';
import type { Live, Outdated } from './hub';
import { channelLabel, type PairedDevice } from './paired';
import { counted } from './words';

/** How a host is doing, in the words every screen uses for it. */
export type HostStatus = {
  /** Connected now; anything shown from an older connection is stale rather than live. */
  online: boolean;
  connecting: boolean;
  /** The snapshot is from a connection that has ended, so it may be out of date. */
  stale: boolean;
  /** The host no longer knows this phone; only forgetting it and joining again helps. */
  unpaired: boolean;
  outdated?: Outdated;
  line: string;
  problem: string | null;
};

export function summary(snapshot: Snapshot): string {
  const agents = snapshot.chats.length;
  const terminals = snapshot.sessions.filter((session) => session.running).length;
  const parts = [];
  if (agents) parts.push(`${agents} ${counted('agent', agents)}`);
  if (terminals) parts.push(`${terminals} ${counted('terminal', terminals)}`);
  return parts.length ? parts.join(' · ') : 'Nothing running';
}

const OUTDATED_LINES: Record<Outdated, string> = { host: 'Needs a newer Sikemux', phone: 'Update this app to connect' };

function offline(device: Pick<PairedDevice, 'lastSeen'> | undefined, now: number): string {
  return `Asleep or offline${device?.lastSeen ? ` · seen ${formatAgo(device.lastSeen, now)}` : ''}`;
}

export function hostStatus(live: Live, device?: Pick<PairedDevice, 'channel' | 'lastSeen'>, now = Date.now()): HostStatus {
  const online = live.status === 'open';
  const stale = !online && !!live.snapshot;
  if (live.status === 'open') {
    const parts = ['Online', channelLabel(device?.channel), live.snapshot ? summary(live.snapshot) : null];
    return { online, connecting: false, stale, unpaired: false, line: parts.filter(Boolean).join(' · '), problem: null };
  }
  if (live.status === 'connecting') {
    // Trying again after a failure still reads as offline, so the line does not flicker on every try.
    const line = live.problem ? offline(device, now) : stale ? 'Reconnecting…' : 'Connecting…';
    return { online, connecting: true, stale, unpaired: false, line, problem: live.problem ?? null };
  }
  const { outdated, problem } = live;
  const unpaired = !!live.unpaired;
  const line = outdated ? OUTDATED_LINES[outdated] : unpaired ? 'No longer knows this phone' : offline(device, now);
  return { online, connecting: false, stale, unpaired, outdated, line, problem };
}
