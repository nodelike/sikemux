import { useEffect } from 'react';
import { router, useIsFocused } from 'expo-router';
import { useAuth } from '@clerk/expo';

import { useFarewell } from '@/account/farewell';
import { useAccountStatus } from '@/account/session';
import { readDevicesAgain, useDevices } from '@/devices/hub';
import { DevicesList } from '@/screens/DevicesList';
import { Farewell } from '@/screens/Farewell';
import { RestoreAccount } from '@/screens/RestoreAccount';
import { Welcome } from '@/screens/Welcome';
import { Button } from '@/ui/controls';
import { alertFailure, PromptScreen } from '@/ui/prompt';

function Unreadable({ problem }: { problem: string }) {
  return (
    <PromptScreen
      icon="IconWarning"
      title="Couldn't read your paired hosts"
      detail={problem}
      actions={
        <Button
          kind="primary"
          title="Try again"
          onPress={() => readDevicesAgain().catch(alertFailure("Couldn't read your paired hosts"))}
        />
      }
    />
  );
}

export { Crashed as ErrorBoundary } from '@/screens/Crashed';

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
