import { expect, it } from 'vitest';

import { runIosReconnectCheck, swiftAvailable } from './iosReconnect.mjs';

it.skipIf(!swiftAvailable)(
  'bounds un-cancellable UniFFI calls in iOS raceTimeout (missed by 7a1d8168f withTaskGroup)',
  () => {
    expect(runIosReconnectCheck()).toBe('ok');
  },
  60_000,
);
