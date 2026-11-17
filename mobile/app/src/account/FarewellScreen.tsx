import { RETENTION } from '@/account/config';
import { sayFarewell, type Farewell as Reason } from '@/account/farewell';
import { type Translate, useT } from '@/i18n';
import { Button } from '@/ui/controls';
import { type Glyph, PromptScreen } from '@/ui/prompt';

function wordsFor(reason: Reason, t: Translate): { icon: Glyph; title: string; body?: string } {
  const restore = t('Sign in again within {days} days to restore it.', { days: RETENTION.restorableDays });
  switch (reason) {
    case 'removed':
      return { icon: 'IconPhone', title: t('This phone was removed from your account') };
    case 'deleted':
      return { icon: 'IconTrash', title: t('Your account was deleted'), body: restore };
    case 'deleted-here':
      return { icon: 'IconTrash', title: t('Your account is deleted'), body: restore };
  }
}

/** Says why the phone signed out, before the welcome screen. */
export function Farewell({ reason }: { reason: Reason }) {
  const t = useT();
  const words = wordsFor(reason, t);
  return (
    <PromptScreen
      icon={words.icon}
      title={words.title}
      detail={words.body}
      actions={<Button kind="primary" title={t('Continue')} onPress={() => sayFarewell(null)} />}
    />
  );
}
