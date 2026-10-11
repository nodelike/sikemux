import { useEffect, useState } from 'react';
import { AppState, Platform } from 'react-native';
import { Stack } from 'expo-router';
import * as SplashScreen from 'expo-splash-screen';
import { StatusBar } from 'expo-status-bar';
import { useFonts } from 'expo-font';
import { Figtree_400Regular, Figtree_400Regular_Italic, Figtree_500Medium, Figtree_600SemiBold } from '@expo-google-fonts/figtree';
import { JetBrainsMono_400Regular } from '@expo-google-fonts/jetbrains-mono';

import { ClerkProvider, useAuth } from '@clerk/expo';
import { resourceCache } from '@clerk/expo/resource-cache';
import { tokenCache } from '@clerk/expo/token-cache';

import { CLERK_PUBLISHABLE_KEY } from '@/account/config';
import { useAccountHostsFeed, useAccountSequence, useConnectArrivals } from '@/account/session';
import { goOffline } from '@/device/identity';
import { useDevices } from '@/devices/hub';
import { NotificationsOffer } from '@/notify/NotificationsOffer';
import { useNotificationResponses } from '@/notify/responses';
import { currentRelays, useUpdateRequired } from '@/network/network';
import { useOverTheAirUpdates } from '@/updates/overTheAir';
import { Crashed } from '@/screens/Crashed';
import { Unreachable } from '@/screens/Unreachable';
import { UpdateRequired } from '@/screens/UpdateRequired';
import { useColors } from '@/ui/theme';

SplashScreen.preventAutoHideAsync().catch(() => {});

/** A notification or link opened at launch still has Devices under it to go back to. */
export const unstable_settings = { initialRouteName: 'index' };

export const ErrorBoundary = Crashed;

/** How long the splash waits for the account before saying it can't be reached. */
const ACCOUNT_WAIT_MS = 10_000;

export default function RootLayout() {
  const colors = useColors();
  // Phones have the fonts built in; only a browser, which runs the showcase, loads them.
  const [loaded, failed] = useFonts(
    Platform.OS === 'web'
      ? { Figtree_400Regular, Figtree_400Regular_Italic, Figtree_500Medium, Figtree_600SemiBold, JetBrainsMono_400Regular }
      : {},
  );

  const required = useUpdateRequired();

  useEffect(() => {
    if (failed || ((loaded || failed) && required)) SplashScreen.hideAsync().catch(() => {});
  }, [loaded, failed, required]);

  useEffect(() => {
    currentRelays().catch(() => {});
    const listener = AppState.addEventListener('change', (state) => {
      if (state === 'active') currentRelays().catch(() => {});
    });
    return () => listener.remove();
  }, []);

  useEffect(() => {
    if (required) goOffline().catch(() => {});
  }, [required]);

  const fontsReady = loaded || failed;
  if (required)
    return fontsReady ? (
      <>
        <StatusBar style="light" />
        <UpdateRequired required={required} />
      </>
    ) : null;
  // The account starts loading at once; only the screens wait for the fonts, so the two load side by side.
  return (
    <ClerkProvider publishableKey={CLERK_PUBLISHABLE_KEY} tokenCache={tokenCache} __experimental_resourceCache={resourceCache}>
      <PhoneOnAccount />
      {fontsReady ? (
        <>
          <StatusBar style="light" />
          <Routes background={colors.ground} />
        </>
      ) : null}
    </ClerkProvider>
  );
}

/** The screens, once the account and the paired hosts are known; the splash stays up until then. */
function Routes({ background }: { background: string }) {
  const { isLoaded, isSignedIn } = useAuth();
  const devices = useDevices();
  const ready = isLoaded && (!isSignedIn || devices.loaded);
  const [late, setLate] = useState(false);

  useEffect(() => {
    if (ready) return;
    const giveUp = setTimeout(() => setLate(true), ACCOUNT_WAIT_MS);
    return () => clearTimeout(giveUp);
  }, [ready]);

  useEffect(() => {
    if (ready || late) SplashScreen.hideAsync().catch(() => {});
  }, [ready, late]);

  if (!ready) return late && !isLoaded ? <Unreachable /> : null;
  return (
    <>
      <Stack screenOptions={{ headerShown: false, freezeOnBlur: true, contentStyle: { backgroundColor: background } }}>
        <Stack.Screen name="index" />
        <Stack.Protected guard={!!isSignedIn}>
          <Stack.Screen name="device/[core]" />
          <Stack.Screen name="join" />
        </Stack.Protected>
      </Stack>
      <NotificationsOffer />
      <NotificationResponses />
    </>
  );
}

/** Mounted with the screens, so a tapped card can open its chat. */
function NotificationResponses() {
  useNotificationResponses();
  return null;
}

function PhoneOnAccount() {
  useOverTheAirUpdates();
  useAccountSequence();
  useAccountHostsFeed();
  useConnectArrivals();
  return null;
}
