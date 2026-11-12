import type { ChatRows } from '@shared/chat/rows';
import type { PlanEntry, AsyncTask, GuestMessage, Handoff, SubagentCard } from '@shared/wire.generated';
import { agentLabel } from '@shared/agents/catalog';
import { groupTasks, subagentActivity, taskDetail, workflowAgentDetail } from '@shared/chat/transcript';
import { counted } from '@/devices/words';
import type { Held } from './outbox';

export type { PlanEntry };

/** What the agent still has going besides the turn, and what waits to be sent. */
export type LiveWork = {
  /** What the next message hands to the agent that took the chat over, or that starts it as a fork. */
  handoff?: Handoff;
  guests: GuestMessage[];
  subagents: SubagentCard[];
  tasks: [string, AsyncTask[]][];
  queued: readonly Held[];
  plan: PlanEntry[];
  /** Some step of the plan is still to do. */
  planOpen: boolean;
};

export function liveWork(chat: Pick<ChatRows, 'handoff' | 'guests' | 'plan' | 'runningSubagents' | 'tasks' | 'queued'>): LiveWork {
  const plan = chat.plan ?? [];
  return {
    handoff: chat.handoff,
    guests: chat.guests ?? [],
    subagents: chat.runningSubagents,
    tasks: groupTasks(chat.tasks),
    queued: chat.queued,
    plan,
    planOpen: plan.some((entry) => entry.status !== 'completed'),
  };
}

/** `task` is the kind an agent named its background tasks, for the pieces that count them. */
export type StripPiece = { key: string; task?: string; agent?: string; count: string; word: string };

const thousands = (tokens: number) => Math.ceil(tokens / 1000);

/** What a handoff's line says first: where the chat goes, or where it was forked from. */
export function handoffTitle(handoff: Handoff, provider: string): string {
  return handoff.forkedFrom === undefined ? `Hands over to ${agentLabel(provider)}` : `Forked from a ${agentLabel(handoff.from)} chat`;
}

/** What a handoff holds, as the Mac's dock line says it. */
export function handoffDetail(handoff: Handoff): string {
  return [
    handoff.request ? 'first request' : undefined,
    handoff.exchanges === 1 ? 'the latest exchange' : `${handoff.exchanges} latest exchanges`,
    handoff.commands ? `${handoff.commands} ${counted('command', handoff.commands)}` : undefined,
    `~${thousands(handoff.tokens)}k tokens`,
  ]
    .filter(Boolean)
    .join(' · ');
}

/** The one line docked on the composer: a count for each kind of work, and the running things it turns through. */
type LiveStripView = { pieces: StripPiece[]; lines: string[]; working: boolean };

export function liveStrip(work: LiveWork): LiveStripView {
  const done = work.plan.filter((entry) => entry.status === 'completed').length;
  const pieces: StripPiece[] = [
    ...(work.handoff ? [{ key: 'handoff', agent: work.handoff.from, count: `~${thousands(work.handoff.tokens)}k`, word: 'handoff' }] : []),
    ...(work.guests.length ? [{ key: 'guests', count: String(work.guests.length), word: 'from guests' }] : []),
    ...(work.subagents.length
      ? [{ key: 'subagents', count: String(work.subagents.length), word: counted('subagent', work.subagents.length) }]
      : []),
    ...work.tasks.map(([kind, tasks]) => ({
      key: `task-${kind}`,
      task: kind,
      count: String(tasks.length),
      word: counted(kind, tasks.length),
    })),
    ...(work.queued.length ? [{ key: 'queued', count: String(work.queued.length), word: 'queued' }] : []),
    ...(work.planOpen ? [{ key: 'plan', count: `${done}/${work.plan.length}`, word: 'plan' }] : []),
  ];
  const running = work.tasks.flatMap(([, tasks]) => tasks.filter((task) => task.state === 'running'));
  const lines = [
    ...work.subagents.map((subagent) => `${subagent.name} · ${subagentActivity(subagent)}`),
    ...running.flatMap((task) => [
      [task.name, taskDetail(task)].filter(Boolean).join(' · '),
      ...(task.agents ?? [])
        .filter((agent) => agent.state === 'running')
        .map((agent) => [agent.label, workflowAgentDetail(agent)].filter(Boolean).join(' · ')),
    ]),
  ];
  return { pieces, lines, working: work.subagents.length > 0 || running.length > 0 };
}

/** What the strip shows, which changes far less often than the chat does. */
export function liveKey(work: LiveWork): string {
  return JSON.stringify(liveStrip(work));
}

export function hasLiveWork(work: LiveWork): boolean {
  return (
    work.handoff !== undefined ||
    work.guests.length > 0 ||
    work.subagents.length > 0 ||
    work.tasks.length > 0 ||
    work.queued.length > 0 ||
    work.planOpen
  );
}
