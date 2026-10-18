import { sayFarewell, type Farewell as Reason } from '@/account/farewell';
import { Button } from '@/ui/controls';
import { type Glyph, PromptScreen } from '@/ui/prompt';

const RESTORE = 'Sign in again within 30 days to restore it.';

const WORDS: Record<Reason, { icon: Glyph; title: string; body?: string }> = {
  removed: { icon: 'IconPhone', title: 'This phone was removed from your account' },
  deleted: { icon: 'IconTrash', title: 'Your account was deleted', body: RESTORE },
  'deleted-here': { icon: 'IconTrash', title: 'Your account is deleted', body: RESTORE },
};

/** Says why the phone signed out, before the welcome screen. */
export function Farewell({ reason }: { reason: Reason }) {
  const words = WORDS[reason];
  return (
    <PromptScreen
      icon={words.icon}
      title={words.title}
      detail={words.body}
      actions={<Button kind="primary" title="Continue" onPress={() => sayFarewell(null)} />}
    />
  );
}
