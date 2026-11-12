import { reloadAppAsync } from 'expo';

import { useT } from '@/i18n';
import { Button } from '@/ui/controls';
import { alertFailure, PromptScreen } from '@/ui/prompt';

/** Shown when the account can't be read at launch: the first launch offline, or the accounts server out of reach. */
export function Unreachable() {
  const t = useT();
  return (
    <PromptScreen
      icon="IconGlobe"
      title={t("Can't reach Sikemux")}
      detail={t('Check the phone is online.')}
      actions={
        <Button kind="primary" title={t('Try again')} onPress={() => reloadAppAsync().catch(alertFailure(t("Couldn't reload the app")))} />
      }
    />
  );
}
