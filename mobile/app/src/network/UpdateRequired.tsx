import type { UpdateRequired as Required } from '@/network/network';
import { updateLink } from '@/network/installed';
import { useT } from '@/i18n';
import { Button } from '@/ui/controls';
import { openLink, PromptScreen } from '@/ui/prompt';

/** The only screen while the installed app is older than the accounts server works with. */
export function UpdateRequired({ required }: { required: Required }) {
  const t = useT();
  return (
    <PromptScreen
      icon="IconDownload"
      title={t('Update Sikemux')}
      detail={t('This version, {current}, is too old. Install {minimum} or later.', {
        current: required.current,
        minimum: required.minimum,
      })}
      actions={<Button kind="primary" title={t('Get the update')} onPress={() => openLink(updateLink())} />}
    />
  );
}
