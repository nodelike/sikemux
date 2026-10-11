import { describe, expect, it } from 'vitest';

import { chooseFactor, confirmed, deletion, START, type DeletionAction, type DeletionState } from './deletion';

function run(state: DeletionState, ...actions: DeletionAction[]): DeletionState {
  return actions.reduce(deletion, state);
}

const EMAIL = { strategy: 'email_code', to: 'c***@nodelike.com' } as const;

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

  it('cannot finish twice or from anywhere but deleting', () => {
    expect(run(START, { type: 'deleted' }).phase.name).toBe('confirm');
  });
});

describe('proving it is them again', () => {
  const deleting = run(START, { type: 'typed', text: 'delete' }, { type: 'delete' });
  const verifying = run(deleting, { type: 'reverify', factor: EMAIL });

  it('asks for a code when the server wants a fresh sign-in, then deletes again once verified', () => {
    expect(verifying.phase).toEqual({ name: 'verify', factor: EMAIL, entry: '', checking: false });
    const checked = run(verifying, { type: 'entered', entry: '123456' }, { type: 'checking' }, { type: 'verified' }, { type: 'deleted' });
    expect(checked.phase.name).toBe('done');
  });

  it('clears a wrong code and says why', () => {
    const wrong = run(
      verifying,
      { type: 'entered', entry: '123456' },
      { type: 'checking' },
      { type: 'rejected', problem: 'Incorrect code' },
    );
    expect(wrong.phase).toEqual({ name: 'verify', factor: EMAIL, entry: '', checking: false });
    expect(wrong.problem).toBe('Incorrect code');
  });

  it('holds the entry while a check is running', () => {
    const checking = run(verifying, { type: 'entered', entry: '123456' }, { type: 'checking' }, { type: 'entered', entry: '1' });
    expect(checking.phase).toMatchObject({ entry: '123456', checking: true });
  });

  it('goes back to confirming when cancelled, without deleting', () => {
    const cancelled = run(verifying, { type: 'cancelled' });
    expect(cancelled.phase.name).toBe('confirm');
    expect(run(cancelled, { type: 'deleted' }).phase.name).toBe('confirm');
  });

  it('does not ask unless it is deleting', () => {
    expect(run(START, { type: 'reverify', factor: EMAIL }).phase.name).toBe('confirm');
  });
});

describe('chooseFactor', () => {
  it('takes the password when the account has one', () => {
    expect(chooseFactor([{ strategy: 'email_code', emailAddressId: 'idn_1' }, { strategy: 'password' }])).toEqual({
      factor: { strategy: 'password' },
    });
  });

  it('sends a code to the email for an account signed in with Google or GitHub', () => {
    expect(chooseFactor([{ strategy: 'email_code', emailAddressId: 'idn_1', safeIdentifier: 'c***@nodelike.com' }])).toEqual({
      factor: EMAIL,
      emailAddressId: 'idn_1',
    });
  });

  it('has nothing to offer without either', () => {
    expect(chooseFactor([{ strategy: 'passkey' }])).toBeUndefined();
  });
});
