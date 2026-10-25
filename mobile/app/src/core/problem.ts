import { MobileError } from '@sikemux/native';

import { errorText } from '@shared/lib/format';

const UNPAIRED = 'This host no longer knows this phone. Forget it, then connect again.';

/** What went wrong, in words for the person: what the host said when it refused, else what the error says. */
export function problem(error: unknown): string {
  if (MobileError.Refused.instanceOf(error) || MobileError.Connection.instanceOf(error)) return error.inner.message;
  if (MobileError.Unpaired.instanceOf(error)) return UNPAIRED;
  return errorText(error);
}
