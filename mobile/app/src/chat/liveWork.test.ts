import { describe, expect, it } from 'vitest';

import { emptyChat } from '@shared/chat/rows';
import type { AsyncTask, SubagentCard } from '@shared/chat/types';
import { hasLiveWork, liveKey, liveWork, planEntries } from './liveWork';

const subagent = (sessionId: string, name: string): SubagentCard => ({ sessionId, name, task: 'look around', state: 'running', calls: 0 });
const task = (asyncTaskId: string, name: string, taskType: string): AsyncTask => ({
  asyncTaskId,
  name,
  taskType,
  description: '',
  state: 'running',
  canStop: true,
});

describe('live work', () => {
  it('reads a plan as its named steps', () => {
    expect(
      planEntries({
        entries: [
          { content: ' Read the tests ', status: 'completed' },
          { content: 'Fix the race', status: 'in_progress' },
          { content: '', status: 'pending' },
          { content: 'Commit', status: 'something new' },
        ],
      }),
    ).toEqual([
      { content: 'Read the tests', status: 'completed' },
      { content: 'Fix the race', status: 'in_progress' },
      { content: 'Commit', status: 'pending' },
    ]);
    expect(planEntries(null)).toEqual([]);
  });

  it('counts running subagents, tasks by kind, waiting messages and an unfinished plan', () => {
    const work = liveWork({
      ...emptyChat,
      runningSubagents: [subagent('a', 'Explore')],
      tasks: [task('t1', 'pnpm dev', 'shell'), task('t2', 'CI', 'monitor'), task('t3', 'tsc -w', 'shell')],
      plan: { entries: [{ content: 'Fix', status: 'completed' }] },
      queued: [{ id: 'q1', text: 'and the docs', paths: [], context: [] }],
    });
    expect(work.subagents.map((subagent) => subagent.name)).toEqual(['Explore']);
    expect(work.tasks.map(([kind, tasks]) => [kind, tasks.length])).toEqual([
      ['shell', 2],
      ['monitor', 1],
    ]);
    expect(work.planOpen).toBe(false);
    expect(hasLiveWork(work)).toBe(true);
    expect(hasLiveWork(liveWork(emptyChat))).toBe(false);
  });

  it('keys the strip on what it counts, not on what streams', () => {
    const working = { ...emptyChat, tasks: [task('t1', 'dev', 'shell')] };
    const said = {
      ...working,
      messages: [{ id: 'm1', role: 'assistant' as const, parts: [{ kind: 'text' as const, id: 'p', text: 'hi' }] }],
    };
    const paused = { ...said, tasks: [{ ...task('t1', 'dev', 'shell'), state: 'paused' as const }] };
    expect(liveKey(liveWork(said))).toBe(liveKey(liveWork(working)));
    expect(liveKey(liveWork(paused))).not.toBe(liveKey(liveWork(working)));
  });
});
