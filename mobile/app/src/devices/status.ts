import { formatAgo } from '@shared/lib/format';
import { english, type Say } from '@shared/lib/say';
import type { Snapshot } from '@/core/protocol';
import type { Live, Outdated } from './hub';
import { channelLabel, type PairedDevice } from './paired';

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
  /** The line without saying it is online, for a screen whose dot already says so. */
  facts: string;
  problem: string | null;
};

export function summary(snapshot: Snapshot, say: Say = english): string {
  const agents = snapshot.chats.length;
  const terminals = snapshot.sessions.filter((session) => session.running).length;
  const parts = [];
  if (agents) parts.push(agents === 1 ? say('1 agent') : say('{count} agents', { count: agents }));
  if (terminals) parts.push(terminals === 1 ? say('1 terminal') : say('{count} terminals', { count: terminals }));
  return parts.length ? parts.join(' · ') : say('Nothing running');
}

function outdatedLine(outdated: Outdated, say: Say): string {
  return outdated === 'host' ? say('Needs a newer Sikemux') : say('Update this app to connect');
}

function offline(device: Pick<PairedDevice, 'lastSeen'> | undefined, now: number, say: Say): string {
  return device?.lastSeen ? say('Asleep or offline · seen {ago}', { ago: formatAgo(device.lastSeen, now, say) }) : say('Asleep or offline');
}

export function hostStatus(
  live: Live,
  device?: Pick<PairedDevice, 'channel' | 'lastSeen'>,
  now = Date.now(),
  say: Say = english,
): HostStatus {
  const online = live.status === 'open';
  const stale = !online && !!live.snapshot;
  if (live.status === 'open') {
    const facts = [channelLabel(device?.channel), live.snapshot ? summary(live.snapshot, say) : null].filter(Boolean).join(' · ');
    const line = [say('Online'), facts].filter(Boolean).join(' · ');
    return { online, connecting: false, stale, unpaired: false, line, facts, problem: null };
  }
  if (live.status === 'connecting') {
    // Trying again after a failure still reads as offline, so the line does not flicker on every try.
    const line = live.problem ? offline(device, now, say) : stale ? say('Reconnecting…') : say('Connecting…');
    return { online, connecting: true, stale, unpaired: false, line, facts: line, problem: live.problem ?? null };
  }
  const { outdated, problem } = live;
  const unpaired = !!live.unpaired;
  const line = outdated ? outdatedLine(outdated, say) : unpaired ? say('No longer knows this phone') : offline(device, now, say);
  return { online, connecting: false, stale, unpaired, outdated, line, facts: line, problem };
}
