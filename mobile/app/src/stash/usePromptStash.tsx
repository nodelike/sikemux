import { useEffect, useState } from 'react';

import { problem } from '@/core/problem';
import { translate } from '@/i18n';
import { haptics } from '@/ui/haptics';
import { StashedToast } from './StashedToast';

/** How long Undo stays offered after a prompt went to the stash. */
const UNDO_MS = 6000;

/**
 * Stashes a composer's prompt with `stash`, which answers the new item's id, and offers Undo for a
 * while after; `toast` is what to show over the composer meanwhile.
 */
export function usePromptStash({
  stash,
  undo,
  report,
}: {
  stash: () => Promise<string>;
  undo: (id: string) => Promise<void>;
  report: (line: string) => void;
}) {
  const [shown, setShown] = useState<{ id?: string } | null>(null);
  useEffect(() => {
    if (!shown?.id) return;
    const gone = setTimeout(() => setShown(null), UNDO_MS);
    return () => clearTimeout(gone);
  }, [shown]);

  const run = () => {
    haptics.tap();
    setShown({});
    stash()
      .then((id) => {
        haptics.success();
        setShown({ id });
      })
      .catch((error: unknown) => {
        setShown(null);
        report(translate('could not stash it: {problem}', { problem: problem(error) }));
      });
  };

  const back = () => {
    const id = shown?.id;
    if (!id) return;
    setShown(null);
    undo(id).catch((error: unknown) => report(translate('could not put it back: {problem}', { problem: problem(error) })));
  };

  return { stash: run, toast: shown ? <StashedToast done={shown.id !== undefined} onUndo={back} /> : null };
}
