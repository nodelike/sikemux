import { useEffect, useMemo } from 'react';
import { Animated, StyleSheet } from 'react-native';
import { Stack, useLocalSearchParams, useNavigation } from 'expo-router';
import SwipeStack from 'expo-router/js-stack';
import { StatusBar } from 'expo-status-bar';

import { useDeviceBackdrop } from '@/devices/backdrop';
import { useDevices, useLive } from '@/devices/hub';
import { useDevicePalette } from '@/devices/palette';
import { Crashed } from '@/ui/Crashed';
import { Backdrop, BackdropContext, backdropUnderStack } from '@/ui/Backdrop';
import { FacesContext } from '@/devices/Buddy';
import { CrewContext, useCrewOf } from '@/devices/Crew';
import { goHome } from '@/ui/navigate';
import { swipeOptions, useStillFade } from '@/ui/swipeStack';
import { isLight, PaletteProvider, type Palette } from '@/ui/theme';

/** A chat opened from a notification still has its host's screen under it to go back to. */
export const unstable_settings = { initialRouteName: 'index' };

export const ErrorBoundary = Crashed;

/** A chat the person just started replaces the new-chat screen in place rather than sliding in. */
function started(route: { params?: object }): boolean {
  return Boolean((route.params as { started?: string } | undefined)?.started);
}

/** A host's screens, drawn in that host's theme and over its pane backdrop. */
export default function DeviceLayout() {
  const { core } = useLocalSearchParams<{ core: string }>();
  const palette = useDevicePalette(core);
  const backdrop = useDeviceBackdrop(core);
  const snapshot = useLive(core).snapshot;
  const agents = snapshot?.workspace.agents;
  const style = snapshot?.faceStyle;
  const faces = useMemo(
    () => (agents && style ? { faces: Object.fromEntries(agents.map((agent) => [agent.id, agent.face ?? null])), style } : undefined),
    [agents, style],
  );
  const crew = useCrewOf(snapshot);
  const navigation = useNavigation();
  // The host's own screen drags back to Devices only from its list; deeper in, the swipe belongs to the screen on top.
  const swipeHostOnlyAtTop = (data: unknown) =>
    navigation.setOptions({ gestureEnabled: ((data as { state?: { index?: number } }).state?.index ?? 0) === 0 });
  const { devices, loaded } = useDevices();
  const paired = !loaded || devices.some((device) => device.core === core);

  useEffect(() => {
    if (!paired) goHome();
  }, [paired]);

  if (!paired) return null;
  return (
    <PaletteProvider value={palette}>
      <BackdropContext.Provider value={backdrop}>
        <FacesContext.Provider value={faces}>
          <CrewContext value={crew}>
            <StatusBar style={isLight(palette) ? 'dark' : 'light'} />
            {backdropUnderStack ? (
              <>
                <StillGround palette={palette} />
                <SwipeStack
                  screenOptions={swipeOptions('transparent')}
                  screenListeners={{ state: (event) => swipeHostOnlyAtTop(event.data) }}>
                  <SwipeStack.Screen
                    name="chat/[agent]"
                    options={({ route }: { route: { params?: object } }) => (started(route) ? { animation: 'none' } : {})}
                  />
                </SwipeStack>
              </>
            ) : (
              <Stack
                screenOptions={{
                  headerShown: false,
                  freezeOnBlur: true,
                  fullScreenGestureEnabled: true,
                  contentStyle: { backgroundColor: palette.ground },
                }}>
                <Stack.Screen name="chat/[agent]" options={({ route }) => ({ animation: started(route) ? 'none' : 'default' })} />
              </Stack>
            )}
          </CrewContext>
        </FacesContext.Provider>
      </BackdropContext.Provider>
    </PaletteProvider>
  );
}

/** The host's ground and backdrop stay put while its screens slide over them, and fade out as the host's screen leaves. */
function StillGround({ palette }: { palette: Palette }) {
  return (
    <Animated.View style={[StyleSheet.absoluteFill, { backgroundColor: palette.ground }, useStillFade()]}>
      <Backdrop />
    </Animated.View>
  );
}
