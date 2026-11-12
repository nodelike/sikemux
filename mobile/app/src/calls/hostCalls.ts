import type { ConnectionLike } from '@sikemux/native';

import { externalValue, useExternalValue, type ExternalValue } from '@shared/lib/externalValue';
import type { AnswerCallParams, PersonCall, ProjectInfo } from '@shared/wire.generated';
import type { Snapshot } from '@/core/protocol';
import { callHost } from '@/core/hostCalls';
import { problem } from '@/core/problem';
import { liveOf } from '@/devices/hub';

/** What the person has done on a call's card while it waits: the options chosen, the note or their words, What runs. */
export type Draft = { chosen?: string[]; typed: string; runsOpen: boolean; sending: boolean };

/** What one host's calls hold beyond its view: what the core took as seen, the call each chat shows, and failures. */
type Kept = {
  /** The calls the core has taken word that this phone drew, which a production call needs before its answer counts. */
  seen: ReadonlySet<string>;
  /** The call a chat shows, by the chat's agent, when the person chose one other than the oldest. */
  focused: Readonly<Record<string, string>>;
  /** Why an answer from a notification did not go, by call. */
  failed: Readonly<Record<string, string>>;
  /** Calls whose chip the person took off the composer, so what it sends goes to the agent instead. */
  composingOff: ReadonlySet<string>;
};

const hosts = new Map<string, ExternalValue<Kept>>();
const drafts = new Map<string, ExternalValue<Draft>>();
const seeing = new Set<string>();

function kept(core: string): ExternalValue<Kept> {
  let found = hosts.get(core);
  if (!found) {
    found = externalValue<Kept>({ seen: new Set(), focused: {}, failed: {}, composingOff: new Set() });
    hosts.set(core, found);
  }
  return found;
}

function change(core: string, update: (now: Kept) => Kept) {
  const store = kept(core);
  store.set(update(store.get()));
}

export function useKept(core: string): Kept {
  return useExternalValue(kept(core));
}

/** The calls waiting on the person, newest first, in `project` or in every project. */
export function openCalls(snapshot: Snapshot | undefined, project?: ProjectInfo): PersonCall[] {
  return (snapshot?.calls ?? [])
    .filter((call) => call.state === 'open' && (!project || call.project === project.path))
    .sort((a, b) => b.raisedAt - a.raisedAt);
}

const settledAt = (call: PersonCall) => call.outcome?.at ?? call.answer?.at ?? call.raisedAt;

/** The calls answered or closed lately, newest first, in `project` or in every project. */
export function settledCalls(snapshot: Snapshot | undefined, project?: ProjectInfo): PersonCall[] {
  return (snapshot?.calls ?? [])
    .filter((call) => call.state !== 'open' && (!project || call.project === project.path))
    .sort((a, b) => settledAt(b) - settledAt(a));
}

/**
 * Every call agent `agentId` waits on the person for, as asker or joined to it, oldest first; or, given `among`, the open
 * calls it names in its order, as the chief's chat docks every call its card from while the person was away counts.
 */
export function queueOf(calls: readonly PersonCall[] | undefined, agentId: string, among?: readonly string[]): PersonCall[] {
  if (among?.length) return among.flatMap((id) => (calls ?? []).filter((call) => call.id === id && call.state === 'open'));
  return (calls ?? [])
    .filter(
      (call) =>
        call.state === 'open' && (call.asker.agentId === agentId || (call.joined ?? []).some((joined) => joined.agentId === agentId)),
    )
    .sort((a, b) => a.raisedAt - b.raisedAt);
}

/** The call agent `agentId`'s chat docks: the one the person chose, while it waits, else the oldest. */
export function waitingCall(queue: PersonCall[], held: Kept, agentId: string): PersonCall | undefined {
  return queue.find((call) => call.id === held.focused[agentId]) ?? queue[0];
}

/**
 * The call whatever a chat's composer sends answers, while one waits there and the person has not taken its chip off.
 * A step to production, or a secret, is answered only on its card.
 */
export function composingCall(call: PersonCall | undefined, held: Kept): PersonCall | undefined {
  if (!call || held.composingOff.has(call.id) || call.kind === 'production' || call.secret) return undefined;
  return call;
}

/** Makes `call` the one the chat of agent `chat` docks. */
export function focus(core: string, call: PersonCall, chat: string) {
  change(core, (now) => ({ ...now, focused: { ...now.focused, [chat]: call.id } }));
}

export function stopComposing(core: string, callId: string) {
  change(core, (now) => ({ ...now, composingOff: new Set([...now.composingOff, callId]) }));
}

/** What the person has done on `call`'s card. Drafts of calls no longer open go. */
export function draftOf(calls: readonly PersonCall[] | undefined, call: PersonCall): ExternalValue<Draft> {
  const held = drafts.get(call.id);
  if (held) return held;
  const open = new Set((calls ?? []).filter((each) => each.state === 'open').map((each) => each.id));
  for (const id of [...drafts.keys()]) if (!open.has(id)) drafts.delete(id);
  const made = externalValue<Draft>({ typed: '', runsOpen: false, sending: false });
  drafts.set(call.id, made);
  return made;
}

export function edit(draft: ExternalValue<Draft>, update: Partial<Draft>) {
  draft.set({ ...draft.get(), ...update });
}

/** The options chosen: until the person touches the rows, the asker's pick. */
export function chosenIn(draft: Draft, call: PersonCall): string[] {
  return draft.chosen ?? (call.pick ? [call.pick] : []);
}

/** Chooses an option, or clears the one chosen; on a call that takes several, ticks it. */
export function choose(draft: ExternalValue<Draft>, call: PersonCall, option: string) {
  const now = chosenIn(draft.get(), call);
  const next = now.includes(option) ? now.filter((id) => id !== option) : call.multi ? [...now, option] : [option];
  edit(draft, { chosen: next });
}

/** What answers the call as the card stands: the options chosen with the note, or the person's own words alone. */
/** A call with nothing to pick that its asker goes on with: Go with this, with nothing typed, lets it stand. */
export const goesOnAsIs = (call: PersonCall) => !!call.assumption && call.options.length === 0;

export function answerOf(draft: Draft, call: PersonCall): AnswerCallParams | undefined {
  const words = draft.typed.trim() || undefined;
  const chosen = chosenIn(draft, call);
  if (!chosen.length) return words ? { id: call.id, words } : goesOnAsIs(call) ? { id: call.id, fine: true } : undefined;
  if (call.multi) return { id: call.id, options: chosen, note: words };
  return { id: call.id, option: chosen[0], note: words };
}

/**
 * Tells the host this phone drew the call, until the core has taken it: a few tries a few seconds apart, and again
 * when the card sees a new connection.
 */
export async function see(core: string, connection: () => ConnectionLike | undefined, call: PersonCall) {
  const key = `${core}/${call.id}`;
  if (kept(core).get().seen.has(call.id) || seeing.has(key)) return;
  seeing.add(key);
  try {
    for (const wait of [0, 2, 5, 10]) {
      if (wait) await new Promise((resolve) => setTimeout(resolve, wait * 1000));
      const now = connection();
      if (!now) return;
      try {
        await callHost(now, 'callSeen', { id: call.id });
        change(core, (held) => ({ ...held, seen: new Set([...held.seen, call.id]) }));
        return;
      } catch (error) {
        console.warn('sikemux: the host did not take that this phone drew a call', error);
      }
    }
  } finally {
    seeing.delete(key);
  }
}

/**
 * Answers a call with what its card holds, once per tap: the card stays sending until the call leaves it. The first
 * answer anywhere wins.
 */
export async function answer(
  core: string,
  connection: ConnectionLike | undefined,
  call: PersonCall,
  draft: ExternalValue<Draft>,
  params: AnswerCallParams,
  report: (text: string) => void,
) {
  if (draft.get().sending || !connection) return;
  edit(draft, { sending: true });
  change(core, (now) => ({ ...now, failed: withoutKey(now.failed, call.id) }));
  try {
    await callHost(connection, 'answerCall', params);
    if (call.secret) {
      edit(draft, { typed: '' });
      drafts.delete(call.id);
    }
  } catch (error) {
    edit(draft, { sending: false });
    report(problem(error));
  }
}

/** Answers the call waiting in a chat with what the person typed in its composer, with the files they sent. */
export async function answerWritten(
  connection: ConnectionLike | undefined,
  callId: string,
  written: { answer: string; paths: string[] },
  report: (text: string) => void,
): Promise<boolean> {
  if (!connection) return false;
  try {
    await callHost(connection, 'answerCall', {
      id: callId,
      words: written.answer,
      paths: written.paths.length ? written.paths : undefined,
    });
    return true;
  } catch (error) {
    report(problem(error));
    return false;
  }
}

/** Hands a call back to its asker's lead, or its asker, to decide, once per tap. */
export async function notMine(
  connection: ConnectionLike | undefined,
  call: PersonCall,
  draft: ExternalValue<Draft>,
  report: (text: string) => void,
) {
  if (draft.get().sending || !connection) return;
  edit(draft, { sending: true });
  try {
    await callHost(connection, 'callNotMine', { id: call.id });
  } catch (error) {
    edit(draft, { sending: false });
    report(problem(error));
  }
}

/**
 * An answer from a notification did not go: its card says so, and why, until it is answered. Notes for calls that no
 * longer wait go with it.
 */
export function answerFailed(core: string, callId: string, why: string) {
  const live = liveOf(core);
  const shown = live.status === 'open' ? live.snapshot : undefined;
  const open = shown && new Set(openCalls(shown).map((call) => call.id));
  if (open && !open.has(callId)) return;
  change(core, (now) => ({
    ...now,
    failed: {
      ...(open ? Object.fromEntries(Object.entries(now.failed).filter(([id]) => open.has(id))) : now.failed),
      [callId]: why,
    },
  }));
}

function withoutKey(record: Readonly<Record<string, string>>, key: string): Record<string, string> {
  const rest = { ...record };
  delete rest[key];
  return rest;
}
