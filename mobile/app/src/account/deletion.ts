import { confirmsDeletion } from '@protocol/account';

export type DeletionPhase = { name: 'confirm' } | { name: 'deleting' } | { name: 'done' };

export type DeletionState = { typed: string; problem?: string; phase: DeletionPhase };

export type DeletionAction =
  { type: 'typed'; text: string } | { type: 'delete' } | { type: 'cancelled' } | { type: 'failed'; problem: string } | { type: 'deleted' };

export const START: DeletionState = { typed: '', phase: { name: 'confirm' } };

export function confirmed(state: Pick<DeletionState, 'typed'>): boolean {
  return confirmsDeletion(state.typed);
}

/** Steps through deleting: confirm by typing the word, delete, done. */
export function deletion(state: DeletionState, action: DeletionAction): DeletionState {
  const phase = state.phase;
  switch (action.type) {
    case 'typed':
      return phase.name === 'confirm' ? { ...state, typed: action.text, problem: undefined } : state;
    case 'delete':
      return phase.name === 'confirm' && confirmed(state) ? { ...state, problem: undefined, phase: { name: 'deleting' } } : state;
    case 'cancelled':
      return phase.name === 'deleting' ? { ...state, problem: undefined, phase: { name: 'confirm' } } : state;
    case 'failed':
      return phase.name === 'deleting' ? { ...state, problem: action.problem, phase: { name: 'confirm' } } : state;
    case 'deleted':
      return phase.name === 'deleting' ? { ...state, problem: undefined, phase: { name: 'done' } } : state;
  }
}
