import { Stack, useLocalSearchParams } from 'expo-router';
import { StatusBar } from 'expo-status-bar';

import { useDeviceBackdrop } from '@/devices/backdrop';
import { useDevicePalette } from '@/devices/palette';
import { BackdropContext } from '@/ui/Backdrop';
import { isLight, PaletteProvider } from '@/ui/theme';

/** A Mac's screens, drawn in that Mac's theme and over its pane backdrop. */
export default function DeviceLayout() {
  const { core } = useLocalSearchParams<{ core: string }>();
  const palette = useDevicePalette(core);
  const backdrop = useDeviceBackdrop(core);
  return (
    <PaletteProvider value={palette}>
      <BackdropContext.Provider value={backdrop}>
        <StatusBar style={isLight(palette) ? 'dark' : 'light'} />
        <Stack screenOptions={{ headerShown: false, contentStyle: { backgroundColor: palette.ground } }} />
      </BackdropContext.Provider>
    </PaletteProvider>
  );
}
