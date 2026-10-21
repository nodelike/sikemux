import { recordOf } from '@mac/lib/guards';
import { groupTasks, runningSubagents, subagentActivity, taskDetail, workflowAgentDetail } from '@mac/chat/transcript';
import type { AcpAsyncTask, AcpSubagent, ChatState } from '@mac/chat/types';
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
  subagents: AcpSubagent[];
  tasks: [string, AcpAsyncTask[]][];
  queued: readonly Held[];
  plan: PlanEntry[];
  /** Some step of the plan is still to do. */
  planOpen: boolean;
};

export function liveWork(state: ChatState, queued: readonly Held[]): LiveWork {
  const plan = planEntries(state.plan);
  return {
    subagents: runningSubagents(state.messages),
    tasks: groupTasks(state.tasks),
    queued,
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
