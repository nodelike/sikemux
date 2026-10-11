import { useEffect } from 'react';
import { Stack, useLocalSearchParams } from 'expo-router';
import { StatusBar } from 'expo-status-bar';

import { useDeviceBackdrop } from '@/devices/backdrop';
import { useDevices } from '@/devices/hub';
import { useDevicePalette } from '@/devices/palette';
import { Crashed } from '@/screens/Crashed';
import { BackdropContext } from '@/ui/Backdrop';
import { goHome } from '@/ui/navigate';
import { isLight, PaletteProvider } from '@/ui/theme';

/** A chat opened from a notification still has its host's screen under it to go back to. */
export const unstable_settings = { initialRouteName: 'index' };

export const ErrorBoundary = Crashed;

/** A host's screens, drawn in that host's theme and over its pane backdrop. */
export default function DeviceLayout() {
  const { core } = useLocalSearchParams<{ core: string }>();
  const palette = useDevicePalette(core);
  const backdrop = useDeviceBackdrop(core);
  const { devices, loaded } = useDevices();
  const paired = !loaded || devices.some((device) => device.core === core);

  useEffect(() => {
    if (!paired) goHome();
  }, [paired]);

  if (!paired) return null;
  return (
    <PaletteProvider value={palette}>
      <BackdropContext.Provider value={backdrop}>
        <StatusBar style={isLight(palette) ? 'dark' : 'light'} />
        <Stack screenOptions={{ headerShown: false, freezeOnBlur: true, contentStyle: { backgroundColor: palette.ground } }} />
      </BackdropContext.Provider>
    </PaletteProvider>
  );
}
