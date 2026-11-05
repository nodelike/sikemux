/*
 * The core's protocol as the phone reads it. What it shows of a host arrives as the core's own JSON, so a field the
 * core adds reaches the phone with an over-the-air update. The types are generated from the core's Rust.
 */

import type { DeviceView, HostInfo, Inbox } from '@shared/wire.generated';

/** Everything the phone shows of one host, as it last sent it. */
export type Snapshot = DeviceView;

export const readView = (json: string) => JSON.parse(json) as Snapshot;

function same(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const keys = Object.keys(a);
  if (keys.length !== Object.keys(b).length) return false;
  return keys.every((key) => same((a as Record<string, unknown>)[key], (b as Record<string, unknown>)[key]));
}

/** The items of `next`, each the previous one with its key when nothing in it changed. */
function keepItems<T>(previous: T[], next: T[], key: (item: T) => string | number): T[] {
  const before = new Map(previous.map((item) => [key(item), item]));
  const kept = next.map((item) => {
    const old = before.get(key(item));
    return old !== undefined && same(old, item) ? old : item;
  });
  return kept.length === previous.length && kept.every((item, index) => item === previous[index]) ? previous : kept;
}

/**
 * A host sends its whole view whenever anything in it changes, several times a second while an agent works. Each
 * part of the new view that equals the last one's is the last one's own object, so a screen or row that shows only
 * unchanged parts is not drawn again.
 */
export function keepUnchanged(previous: Snapshot | undefined, next: Snapshot): Snapshot {
  if (!previous) return next;
  const keep = <K extends keyof Snapshot>(key: K): Snapshot[K] => (same(previous[key], next[key]) ? previous[key] : next[key]);
  return {
    ...next,
    workspace: keep('workspace'),
    layout: keep('layout'),
    account: keep('account'),
    faceStyle: keep('faceStyle'),
    load: keep('load'),
    updates: keep('updates'),
    sessions: keepItems(previous.sessions, next.sessions, (session) => session.id),
    chats: keepItems(previous.chats, next.chats, (chat) => chat.agentId),
    attentions: keepItems(previous.attentions, next.attentions, (attention) => attention.id),
    recent: keepItems(previous.recent, next.recent, (chat) => chat.id),
    inbox: keepInbox(previous.inbox, next.inbox),
  };
}

function keepInbox(previous: Inbox, next: Inbox): Inbox {
  const kept: Inbox = {
    ...next,
    items: keepItems(previous.items, next.items, (item) => item.id),
    remoteAgents: keepItems(previous.remoteAgents, next.remoteAgents, (agent) => agent.agentId),
    agents: keepItems(previous.agents, next.agents, (agent) => agent.agentId),
    projects: same(previous.projects, next.projects) ? previous.projects : next.projects,
    needs: same(previous.needs, next.needs) ? previous.needs : next.needs,
    rollup: same(previous.rollup, next.rollup) ? previous.rollup : next.rollup,
    agentMessages: same(previous.agentMessages, next.agentMessages) ? previous.agentMessages : next.agentMessages,
  };
  const keys = new Set([...Object.keys(previous), ...Object.keys(kept)]) as Set<keyof Inbox>;
  return [...keys].every((key) => kept[key] === previous[key]) ? previous : kept;
}

export const readHost = (json: string) => JSON.parse(json) as HostInfo;
