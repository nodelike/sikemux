import { useEffect } from 'react';
import { StyleSheet, Text, View } from 'react-native';
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
import { Screen, useBottomGap } from '@/ui/screen';
import { typeFor, useStyles, type Palette } from '@/ui/theme';

function Unreadable({ problem }: { problem: string }) {
  const styles = useStyles(makeStyles);
  const bottom = useBottomGap();
  return (
    <Screen>
      <View style={styles.block}>
        <Text style={styles.title}>Couldn&apos;t read your paired hosts</Text>
        <Text style={styles.body}>{problem}</Text>
      </View>
      <View style={[styles.footer, { paddingBottom: bottom }]}>
        <Button kind="primary" title="Try again" onPress={() => readDevicesAgain().catch(() => {})} />
      </View>
    </Screen>
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

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    block: { flex: 1, justifyContent: 'center', paddingHorizontal: 32, paddingBottom: 120 },
    title: { ...type.title, fontSize: 20, textAlign: 'center' },
    body: { ...type.body, textAlign: 'center', marginTop: 8 },
    footer: { paddingHorizontal: 16, paddingTop: 12 },
  });
};
