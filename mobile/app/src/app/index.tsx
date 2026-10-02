import { StyleSheet, Text, View } from 'react-native';

import { reloadDevices, useDevices } from '@/devices/hub';
import { DevicesList } from '@/screens/DevicesList';
import { Welcome } from '@/screens/Welcome';
import { Button, Screen, useBottomGap } from '@/ui/parts';
import { typeFor, useStyles, type Palette } from '@/ui/theme';

function Unreadable({ problem }: { problem: string }) {
  const styles = useStyles(makeStyles);
  const bottom = useBottomGap();
  return (
    <Screen>
      <View style={styles.block}>
        <Text style={styles.title}>Couldn&apos;t read your paired Macs</Text>
        <Text style={styles.body}>{problem}</Text>
      </View>
      <View style={[styles.footer, { paddingBottom: bottom }]}>
        <Button kind="primary" title="Try again" onPress={() => reloadDevices().catch(() => {})} />
      </View>
    </Screen>
  );
}

export default function Home() {
  const { devices, loaded, problem } = useDevices();
  if (problem) return <Unreadable problem={problem} />;
  if (!loaded) return null;
  return devices.length ? <DevicesList devices={devices} /> : <Welcome />;
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
