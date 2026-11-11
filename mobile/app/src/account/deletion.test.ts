import { describe, expect, it } from 'vitest';

import { confirmed, deletion, START, type DeletionAction, type DeletionState } from './deletion';

function run(state: DeletionState, ...actions: DeletionAction[]): DeletionState {
  return actions.reduce(deletion, state);
}

describe('confirming', () => {
  it('needs the word delete typed before it deletes', () => {
    expect(run(START, { type: 'typed', text: 'del' }, { type: 'delete' }).phase.name).toBe('confirm');
    expect(run(START, { type: 'typed', text: ' Delete ' }, { type: 'delete' }).phase.name).toBe('deleting');
    expect(confirmed({ typed: '' })).toBe(false);
  });

  it('keeps what was typed and stops editing it while deleting', () => {
    const deleting = run(START, { type: 'typed', text: 'delete' }, { type: 'delete' });
    expect(run(deleting, { type: 'typed', text: 'x' }).typed).toBe('delete');
  });
});

describe('deleting', () => {
  const deleting = run(START, { type: 'typed', text: 'delete' }, { type: 'delete' });

  it('goes back to confirming with the problem when it fails', () => {
    const failed = run(deleting, { type: 'failed', problem: "Can't reach Sikemux." });
    expect(failed.phase.name).toBe('confirm');
    expect(failed.problem).toBe("Can't reach Sikemux.");
    expect(failed.typed).toBe('delete');
    expect(run(failed, { type: 'typed', text: 'delete ' }).problem).toBeUndefined();
  });

  it('goes back to confirming when the person backs out, without deleting', () => {
    const cancelled = run(deleting, { type: 'cancelled' });
    expect(cancelled.phase.name).toBe('confirm');
    expect(run(cancelled, { type: 'deleted' }).phase.name).toBe('confirm');
  });
});
