import { reloadAppAsync } from 'expo';

import { Button } from '@/ui/controls';
import { alertFailure, PromptScreen } from '@/ui/prompt';

/** Shown when the account can't be read at launch: the first launch offline, or the accounts server out of reach. */
export function Unreachable() {
  return (
    <PromptScreen
      icon="IconGlobe"
      title="Can't reach Sikemux"
      detail="Check the phone is online."
      actions={<Button kind="primary" title="Try again" onPress={() => reloadAppAsync().catch(alertFailure("Couldn't reload the app"))} />}
    />
  );
}
