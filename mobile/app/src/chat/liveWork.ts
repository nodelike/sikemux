import { recordOf } from '@shared/lib/guards';
import type { ChatRows } from '@shared/chat/rows';
import { groupTasks, subagentActivity, taskDetail, workflowAgentDetail } from '@shared/chat/transcript';
import type { AsyncTask, SubagentCard } from '@shared/chat/types';
import { counted } from '@/devices/words';
import type { Held } from './session';

export type PlanEntry = { content: string; status: 'pending' | 'in_progress' | 'completed' };

/** The checklist an agent keeps for its turn, as the steps it names. */
export function planEntries(plan: unknown): PlanEntry[] {
  const entries = recordOf(plan)?.entries;
  if (!Array.isArray(entries)) return [];
  return entries.flatMap((entry): PlanEntry[] => {
    const record = recordOf(entry);
    const content = typeof record?.content === 'string' ? record.content.trim() : '';
    if (!content) return [];
    const status = record?.status === 'completed' || record?.status === 'in_progress' ? record.status : 'pending';
    return [{ content, status }];
  });
}

/** What the agent still has going besides the turn, and what waits to be sent. */
export type LiveWork = {
  subagents: SubagentCard[];
  tasks: [string, AsyncTask[]][];
  queued: readonly Held[];
  plan: PlanEntry[];
  /** Some step of the plan is still to do. */
  planOpen: boolean;
};

export function liveWork(chat: Pick<ChatRows, 'plan' | 'runningSubagents' | 'tasks' | 'queued'>): LiveWork {
  const plan = planEntries(chat.plan);
  return {
    subagents: chat.runningSubagents,
    tasks: groupTasks(chat.tasks),
    queued: chat.queued,
    plan,
    planOpen: plan.some((entry) => entry.status !== 'completed'),
  };
}

/** `task` is the kind an agent named its background tasks, for the pieces that count them. */
export type StripPiece = { key: string; task?: string; count: string; word: string };

/** The one line docked on the composer: a count for each kind of work, and the running things it turns through. */
export type LiveStripView = { pieces: StripPiece[]; lines: string[]; working: boolean };

export function liveStrip(work: LiveWork): LiveStripView {
  const done = work.plan.filter((entry) => entry.status === 'completed').length;
  const pieces: StripPiece[] = [
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
  return work.subagents.length > 0 || work.tasks.length > 0 || work.queued.length > 0 || work.planOpen;
}
