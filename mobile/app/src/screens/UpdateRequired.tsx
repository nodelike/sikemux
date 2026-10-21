import type { UpdateRequired as Required } from '@/network/network';
import { updateLink } from '@/network/installed';
import { Button } from '@/ui/controls';
import { openLink, PromptScreen } from '@/ui/prompt';

/** The only screen while the installed app is older than the accounts server works with. */
export function UpdateRequired({ required }: { required: Required }) {
  return (
    <PromptScreen
      icon="IconDownload"
      title="Update Sikemux"
      detail={`This version, ${required.current}, is too old. Install ${required.minimum} or later.`}
      actions={<Button kind="primary" title="Get the update" onPress={() => openLink(updateLink())} />}
    />
  );
}
