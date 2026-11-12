import { useEffect } from 'react';
import { useAuth } from '@clerk/expo';

import { callHost } from '@/core/hostCalls';
import { knownDevices, liveOf, onAnyHost, openConnections } from '@/devices/hub';
import { deviceName } from '@/devices/paired';
import { lockOn, onLockChange } from '@/lock/lock';
import { formatAge } from '@shared/lib/format';
import type { AccountUsage, UsageResetAt } from '@shared/wire.generated';

import { notifier } from '../../modules/notify';
import { widgets, type LimitLine } from '../../modules/widgets';

/** Hosts change their agents in bursts; the shade follows once they settle. */
const SETTLE_MS = 1000;
/** Limits change slowly, and the Mac reads them this often too. */
const LIMITS_EVERY_MS = 5 * 60_000;

type AtWork = { title: string; text: string | null; url: string | null };

const usages = new Map<string, AccountUsage[]>();
const usagesRead = new Map<string, number>();
let shown: string | undefined;
let shownLimits: string | undefined;

/** Who needs the person or works now across hosts, in the shade's words; none while nothing does. */
function atWork(): AtWork | undefined {
  let needed = 0;
  let working = 0;
  let first: { core: string; host: string; agentId: string; title: string; name?: string | null; needs: boolean } | undefined;
  for (const device of knownDevices()) {
    const inbox = liveOf(device.core).snapshot?.inbox;
    if (!inbox) continue;
    needed += inbox.needed ?? inbox.items.filter((item) => item.need).length;
    working += inbox.working;
    const lead = inbox.agents.find((agent) => agent.group === 'blocked') ?? inbox.agents.find((agent) => agent.group === 'working');
    if (lead && (!first || (!first.needs && lead.group === 'blocked'))) {
      first = {
        core: device.core,
        host: deviceName(device),
        agentId: lead.agentId,
        title: lead.title,
        name: lead.name,
        needs: lead.group === 'blocked',
      };
    }
  }
  if (needed === 0 && working === 0) return undefined;
  const hidden = lockOn();
  const who = first?.needs ? (hidden || !first.name ? 'An agent needs you' : `@${first.name} needs you`) : null;
  const more = who && needed > 1 ? `${needed - 1} more` : null;
  const needs = !who && needed > 0 ? `${needed} need you` : null;
  return {
    title: [who, more, needs, working > 0 ? `${working} working` : null].filter(Boolean).join(' · '),
    text: first && !hidden ? `${first.title} · ${first.host}` : null,
    url: first ? `sikemux://device/${first.core}/chat/${first.agentId}` : null,
  };
}

function resetsAt(at: UsageResetAt | null | undefined): number | undefined {
  if (at == null) return undefined;
  return typeof at === 'number' ? at * 1000 : Date.parse(at) || undefined;
}

/** Every signed-in account's windows once, however many hosts share it, from the newest reading. */
function limitLines(): LimitLine[] {
  const newest = new Map<string, AccountUsage>();
  for (const device of knownDevices()) {
    for (const account of usages.get(device.core) ?? []) {
      if (!account.signedIn || !account.usage) continue;
      const key = `${account.provider}/${account.label}`;
      if ((account.readAt ?? 0) >= (newest.get(key)?.readAt ?? -1)) newest.set(key, account);
    }
  }
  const now = Date.now();
  return [...newest.values()]
    .sort((a, b) => a.provider.localeCompare(b.provider) || a.label.localeCompare(b.label))
    .flatMap((account) =>
      (account.usage?.windows ?? []).map((window) => {
        const at = resetsAt(window.resetsAt);
        return {
          label: `${account.label} · ${window.scope ? `${window.label} · ${window.scope}` : window.label}`,
          used: window.usedPercent,
          resets: at === undefined ? '' : formatAge(now, at),
        };
      }),
    );
}

async function readLimits() {
  const now = Date.now();
  await Promise.all(
    openConnections().map(async ([core, connection]) => {
      if (now - (usagesRead.get(core) ?? 0) < LIMITS_EVERY_MS) return;
      usagesRead.set(core, now);
      try {
        usages.set(core, await callHost(connection, 'agentUsages', {}));
      } catch (error) {
        console.warn(`sikemux: could not read the plan limits of ${core.slice(0, 8)}`, error);
      }
    }),
  );
}

async function write() {
  await readLimits();
  const next = atWork();
  const key = JSON.stringify(next ?? null);
  if (key !== shown) {
    shown = key;
    if (next) notifier?.showAgents(next.title, next.text, next.url);
    else notifier?.cancelAgents();
  }
  const lines = limitLines();
  const limitsKey = JSON.stringify(lines);
  if (lines.length > 0 && limitsKey !== shownLimits) {
    shownLimits = limitsKey;
    widgets?.setLimits(lines);
  }
}

function forget() {
  shown = undefined;
  shownLimits = undefined;
  usages.clear();
  usagesRead.clear();
  notifier?.cancelAgents();
  widgets?.setLimits([]);
}

/** Keeps the shade's agents-at-work notification and the Limits widget in step with the hosts while signed in. */
export function useGlanceFeed() {
  const { isSignedIn } = useAuth();
  useEffect(() => {
    if (!isSignedIn) return forget;
    let settling: ReturnType<typeof setTimeout> | undefined;
    const changed = () => {
      clearTimeout(settling);
      settling = setTimeout(() => void write(), SETTLE_MS);
    };
    changed();
    const offHosts = onAnyHost(changed);
    const offLock = onLockChange(changed);
    return () => {
      clearTimeout(settling);
      offHosts();
      offLock();
    };
  }, [isSignedIn]);
}
