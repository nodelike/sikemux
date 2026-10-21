import { describe, expect, it } from 'vitest';

import { chooseFactor } from '@protocol/account';

import { verifying, type Verify, type VerifyAction } from './reverify';

function run(state: Verify | undefined, ...actions: VerifyAction[]): Verify | undefined {
  return actions.reduce(verifying, state);
}

const EMAIL = { strategy: 'email_code', to: 'c***@nodelike.com', emailAddressId: 'idn_1' } as const;

describe('proving it is them again', () => {
  const asked = run(undefined, { type: 'ask', factor: EMAIL });

  it('asks for a code when the server wants a fresh sign-in, and is done once verified', () => {
    expect(asked).toEqual({ factor: EMAIL, entry: '', checking: false });
    expect(run(asked, { type: 'entered', entry: '123456' }, { type: 'checking' }, { type: 'done' })).toBeUndefined();
  });

  it('clears a wrong code and says why', () => {
    const wrong = run(asked, { type: 'entered', entry: '123456' }, { type: 'checking' }, { type: 'rejected', problem: 'Incorrect code' });
    expect(wrong).toEqual({ factor: EMAIL, entry: '', checking: false, problem: 'Incorrect code' });
    expect(run(wrong, { type: 'entered', entry: '1' })?.problem).toBeUndefined();
  });

  it('holds the entry while a check is running', () => {
    const checking = run(asked, { type: 'entered', entry: '123456' }, { type: 'checking' }, { type: 'entered', entry: '1' });
    expect(checking).toMatchObject({ entry: '123456', checking: true });
  });
});

describe('chooseFactor', () => {
  it('takes the password when the account has one', () => {
    expect(chooseFactor([{ strategy: 'email_code', emailAddressId: 'idn_1' }, { strategy: 'password' }])).toEqual({ strategy: 'password' });
  });

  it('sends a code to the email for an account signed in with Google or GitHub', () => {
    expect(chooseFactor([{ strategy: 'email_code', emailAddressId: 'idn_1', safeIdentifier: 'c***@nodelike.com' }])).toEqual(EMAIL);
  });

  it('has nothing to offer without either', () => {
    expect(chooseFactor([{ strategy: 'passkey' }])).toBeUndefined();
  });
});
