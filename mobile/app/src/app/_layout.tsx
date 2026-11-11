import { useEffect, useState } from 'react';
import { AppState, Platform, View } from 'react-native';
import { Stack } from 'expo-router';
import SwipeStack from 'expo-router/js-stack';
import * as SplashScreen from 'expo-splash-screen';
import { StatusBar } from 'expo-status-bar';
import { KeyboardProvider } from 'react-native-keyboard-controller';
import { useFonts } from 'expo-font';
import { Figtree_400Regular, Figtree_400Regular_Italic, Figtree_500Medium, Figtree_600SemiBold } from '@expo-google-fonts/figtree';
import { JetBrainsMono_400Regular } from '@expo-google-fonts/jetbrains-mono';

import { ClerkProvider, useAuth } from '@clerk/expo';
import { resourceCache } from '@clerk/expo/resource-cache';

import { CLERK_PUBLISHABLE_KEY } from '@/account/config';
import { tokenCache } from '@/account/tokenCache';
import { useAccountHostsFeed, useAccountSequence } from '@/account/session';
import { useHostJoins } from '@/devices/hostJoins';
import { goOffline } from '@/phone/identity';
import { useDevices } from '@/devices/hub';
import { lockFor, useLock } from '@/lock/lock';
import { LockCover, LockScreen } from '@/lock/LockScreen';
import { NotificationsOffer } from '@/notify/NotificationsOffer';
import { currentRelays, useUpdateRequired } from '@/network/network';
import { useOverTheAirUpdates } from '@/updates/overTheAir';
import { Crashed } from '@/ui/Crashed';
import { appReady, useCrashReportsAccount, withCrashReports } from '@/crashes/reports';
import { Unreachable } from '@/account/Unreachable';
import { useShareIntake } from '@/stash/incoming';
import { useStashFeed } from '@/stash/store';
import { UpdateRequired } from '@/network/UpdateRequired';
import { backdropUnderStack } from '@/ui/Backdrop';
import { swipeOptions } from '@/ui/swipeStack';
import { useColors } from '@/ui/theme';

SplashScreen.preventAutoHideAsync().catch(() => {});

/** A notification or link opened at launch still has Devices under it to go back to. */
export const unstable_settings = { initialRouteName: 'index' };

export const ErrorBoundary = Crashed;

/** How long the splash waits for the account before saying it can't be reached. */
const ACCOUNT_WAIT_MS = 10_000;

export default withCrashReports(RootLayout);

function RootLayout() {
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
        <KeyboardProvider>
          <StatusBar style="light" />
          <Routes background={colors.ground} />
        </KeyboardProvider>
      ) : null}
    </ClerkProvider>
  );
}

/** The screens, once the account and the paired hosts are known; the splash stays up until then. */
function Routes({ background }: { background: string }) {
  const { isLoaded, isSignedIn, userId } = useAuth();
  useCrashReportsAccount(userId);
  const devices = useDevices();
  const ready = isLoaded && (!isSignedIn || devices.loaded);
  const [late, setLate] = useState(false);
  const lock = useLock();

  useEffect(() => {
    if (ready) return;
    const giveUp = setTimeout(() => setLate(true), ACCOUNT_WAIT_MS);
    return () => clearTimeout(giveUp);
  }, [ready]);

  useEffect(() => {
    if (!ready && !late) return;
    SplashScreen.hideAsync().catch(() => {});
    appReady();
  }, [ready, late]);

  useEffect(() => {
    if (isLoaded) void lockFor(!!isSignedIn);
  }, [isLoaded, isSignedIn]);

  if (!ready) return late && !isLoaded ? <Unreachable /> : null;
  if (isSignedIn && !lock.opened) return lock.on ? <LockScreen /> : null;
  return (
    <>
      {backdropUnderStack ? (
        <View style={{ flex: 1, backgroundColor: background }}>
          <SwipeStack screenOptions={swipeOptions(background)}>
            <SwipeStack.Screen name="index" />
            <SwipeStack.Protected guard={!!isSignedIn}>
              {/* A host's screen paints its own ground, which fades as the screen leaves. */}
              <SwipeStack.Screen name="device/[core]" options={{ cardStyle: { backgroundColor: 'transparent' } }} />
              <SwipeStack.Screen name="host-sign-in" />
              <SwipeStack.Screen name="share/[id]" />
              <SwipeStack.Screen name="stash" />
            </SwipeStack.Protected>
          </SwipeStack>
        </View>
      ) : (
        <Stack
          screenOptions={{
            headerShown: false,
            freezeOnBlur: true,
            fullScreenGestureEnabled: true,
            contentStyle: { backgroundColor: background },
          }}>
          <Stack.Screen name="index" />
          <Stack.Protected guard={!!isSignedIn}>
            <Stack.Screen name="device/[core]" />
            <Stack.Screen name="host-sign-in" />
            <Stack.Screen name="share/[id]" />
            <Stack.Screen name="stash" />
          </Stack.Protected>
        </Stack>
      )}
      {!isSignedIn || lock.phase === 'open' ? (
        <>
          <NotificationsOffer />
          {isSignedIn ? <ShareIntake /> : null}
        </>
      ) : null}
      {isSignedIn ? <LockCover /> : null}
    </>
  );
}

/** Mounted while signed in and unlocked, so a share opens its sheet only then. */
function ShareIntake() {
  useShareIntake();
  return null;
}

function PhoneOnAccount() {
  useOverTheAirUpdates();
  useAccountSequence();
  useAccountHostsFeed();
  useStashFeed();
  useHostJoins();
  return null;
}
