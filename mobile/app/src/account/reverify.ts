import type { Factor } from '@protocol/account';

/** The proof being asked for, while the server wants a sign-in from the last ten minutes. */
export type Verify = { factor: Factor; entry: string; checking: boolean; problem?: string };

export type VerifyAction =
  | { type: 'ask'; factor: Factor }
  | { type: 'entered'; entry: string }
  | { type: 'checking' }
  | { type: 'rejected'; problem: string }
  | { type: 'done' };

/** Steps through proving it is them: asked, typing, checking, a wrong entry cleared with why, then done. */
export function verifying(state: Verify | undefined, action: VerifyAction): Verify | undefined {
  switch (action.type) {
    case 'ask':
      return { factor: action.factor, entry: '', checking: false };
    case 'entered':
      return state && !state.checking ? { ...state, entry: action.entry, problem: undefined } : state;
    case 'checking':
      return state ? { ...state, checking: true, problem: undefined } : state;
    case 'rejected':
      return state ? { ...state, entry: '', checking: false, problem: action.problem } : state;
    case 'done':
      return undefined;
  }
}
