import type { RevokeReason } from '@sikemux/protocol';

import { externalValue, useExternalValue } from '@shared/lib/externalValue';

/** Why this phone signed out: removed from the account, the account deleted elsewhere, or deleted here. */
export type Farewell = 'removed' | 'deleted' | 'deleted-here';

const shown = externalValue<Farewell | null>(null);

export function sayFarewell(farewell: Farewell | null) {
  // The live connection also hears of a deletion made here, and may hear it last.
  if (shown.get() === 'deleted-here' && farewell === 'deleted') return;
  shown.set(farewell);
}

/** Signing out on this phone needs no explanation; anything else does. */
export function farewellFor(reason: RevokeReason): Farewell | null {
  if (reason === 'removed') return 'removed';
  if (reason === 'account_deleted') return 'deleted';
  return null;
}

export function useFarewell(): Farewell | null {
  return useExternalValue(shown);
}
