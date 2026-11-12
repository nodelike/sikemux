import { useEffect } from 'react';
import { router, useIsFocused } from 'expo-router';
import { useAuth } from '@clerk/expo';

import { useFarewell } from '@/account/farewell';
import { useAccountStatus } from '@/account/session';
import { readDevicesAgain, useDevices } from '@/devices/hub';
import { DevicesList } from '@/devices/DevicesList';
import { Farewell } from '@/account/FarewellScreen';
import { RestoreAccount } from '@/account/RestoreAccount';
import { Welcome } from '@/account/Welcome';
import { useT } from '@/i18n';
import { Button } from '@/ui/controls';
import { alertFailure, PromptScreen } from '@/ui/prompt';

function Unreadable({ problem }: { problem: string }) {
  const t = useT();
  return (
    <PromptScreen
      icon="IconWarning"
      title={t("Couldn't read your paired hosts")}
      detail={problem}
      actions={
        <Button
          kind="primary"
          title={t('Try again')}
          onPress={() => readDevicesAgain().catch(alertFailure(t("Couldn't read your paired hosts")))}
        />
      }
    />
  );
}

export { Crashed as ErrorBoundary } from '@/ui/Crashed';

/** Whether launch has decided between Devices and the one host; it opens that host only once. */
let launched = false;

export default function Home() {
  const { isSignedIn } = useAuth();
  const { devices, problem } = useDevices();
  const farewell = useFarewell();
  const status = useAccountStatus();
  const focused = useIsFocused();
  const only = isSignedIn && devices.length === 1 ? devices[0].core : undefined;

  useEffect(() => {
    if (launched || !isSignedIn) return;
    launched = true;
    if (focused && only) router.push(`/device/${only}`);
  }, [isSignedIn, focused, only]);

  if (!isSignedIn) return farewell ? <Farewell reason={farewell} /> : <Welcome />;
  if (status?.step === 'deleting') return <RestoreAccount deletesAt={status.deletesAt} />;
  if (problem) return <Unreadable problem={problem} />;
  return <DevicesList devices={devices} />;
}
