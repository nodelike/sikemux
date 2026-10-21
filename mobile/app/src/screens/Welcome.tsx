import { useEffect, useState } from 'react';
import { AccessibilityInfo, Animated, Easing, Platform, Pressable, StyleSheet, Text, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { router } from 'expo-router';
import { useSSO } from '@clerk/expo';
import { useSignInWithApple } from '@clerk/expo/apple';
import { useSignInWithGoogle } from '@clerk/expo/google';
import Constants from 'expo-constants';
import * as AuthSession from 'expo-auth-session';
import * as WebBrowser from 'expo-web-browser';
import Svg, { Defs, LinearGradient, Rect, Stop } from 'react-native-svg';

import { problem as problemOf } from '@/core/problem';
import { AppleMark, GitHubMark, GoogleG } from '@/ui/brands';
import { AgentIcon, Icon } from '@/ui/Icon';
import { Button } from '@/ui/controls';
import { useBottomGap } from '@/ui/screen';
import { Dot, NeedsYou, Working } from '@/ui/status';
import { defaultPalette as colors, fonts, typeFor } from '@/ui/theme';

WebBrowser.maybeCompleteAuthSession();

type Provider = 'oauth_apple' | 'oauth_google' | 'oauth_github';

// Shown before any host is paired, so it is drawn in the default theme.
const type = typeFor(colors);

/** What the app is for, before there is anything of the person's to show: the rail, drifting past. */
const REEL = [
  { provider: 'claude', title: 'Tighten the rail spacing', place: 'sikemux', doing: 'Editing…', state: 'working' },
  { provider: 'codex', title: 'Fix the flaky PTY replay test', place: 'sikemux', doing: 'Running a command…', state: 'working' },
  { provider: 'claude', title: 'Remote pairing screen', place: 'sikemux', doing: 'Needs input', state: 'needs' },
  { provider: 'hermes', title: 'Release notes for v0.4.3', place: 'sikemux-front', doing: 'Done', state: 'done' },
  { provider: 'opencode', title: 'Why does the shader dim the rail', place: 'sikemux', doing: 'Thinking…', state: 'working' },
  { provider: 'codex', title: 'Download size on the hero', place: 'sikemux-front', doing: 'Reading…', state: 'working' },
] as const;

const ROW = 56;
const REEL_HEIGHT = 340;

function ReelRow({ row }: { row: (typeof REEL)[number] }) {
  return (
    <View style={styles.row}>
      <AgentIcon provider={row.provider} size={22} />
      <View style={{ flex: 1 }}>
        <Text style={[type.row, styles.rowTitle]} numberOfLines={1}>
          {row.title}
        </Text>
        <View style={styles.rowPlace}>
          <Icon name="IconFolder" size={11} color={colors.live} />
          <Text style={styles.rowDetail}>{`${row.place} · ${row.doing}`}</Text>
        </View>
      </View>
      <View style={styles.state}>
        {row.state === 'working' ? <Working /> : row.state === 'needs' ? <NeedsYou /> : <Dot color={colors.accent} />}
      </View>
    </View>
  );
}

function Fade({ edge }: { edge: 'top' | 'bottom' }) {
  const id = `fade-${edge}`;
  return (
    <Svg style={[styles.fade, edge === 'top' ? { top: 0 } : { bottom: 0 }]} pointerEvents="none">
      <Defs>
        <LinearGradient id={id} x1="0" y1="0" x2="0" y2="1">
          <Stop offset="0" stopColor={colors.ground} stopOpacity={edge === 'top' ? 1 : 0} />
          <Stop offset="1" stopColor={colors.ground} stopOpacity={edge === 'top' ? 0 : 1} />
        </LinearGradient>
      </Defs>
      <Rect width="100%" height="100%" fill={`url(#${id})`} />
    </Svg>
  );
}

function Reel() {
  const [drift] = useState(() => new Animated.Value(0));
  const [still, setStill] = useState(false);
  useEffect(() => {
    AccessibilityInfo.isReduceMotionEnabled().then(setStill);
  }, []);
  useEffect(() => {
    if (still) return;
    const loop = Animated.loop(Animated.timing(drift, { toValue: 1, duration: 26_000, easing: Easing.linear, useNativeDriver: true }));
    loop.start();
    return () => loop.stop();
  }, [drift, still]);
  const translateY = drift.interpolate({ inputRange: [0, 1], outputRange: [0, -ROW * REEL.length] });
  return (
    <View style={styles.reel} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      <Animated.View style={{ transform: [{ translateY }] }}>
        {[...REEL, ...REEL].map((row, index) => (
          <ReelRow key={index} row={row} />
        ))}
      </Animated.View>
      <Fade edge="top" />
      <Fade edge="bottom" />
    </View>
  );
}

/** Builds with their own Google clients sign in natively; the others use Google's page in an in-app sheet. */
const NATIVE_GOOGLE = Boolean(Constants.expoConfig?.extra?.EXPO_PUBLIC_CLERK_GOOGLE_WEB_CLIENT_ID);

/** Apple's sheet fails with codes like ERR_REQUEST_UNKNOWN, even when the iPhone has no Apple Account to sign in with. */
function describe(error: unknown): string {
  const code = (error as { code?: unknown } | undefined)?.code;
  if (typeof code === 'string' && code.startsWith('ERR_REQUEST'))
    return 'Apple did not sign you in. Check this iPhone is signed in to an Apple Account in Settings, then try again.';
  return problemOf(error);
}

/** Apple signs in with the system's sheet, so it is offered only on iPhones. */
const APPLE = Platform.OS === 'ios';

/**
 * Apple and Google sign in with the system's own sheets where this build can; GitHub, which has no
 * native sign-in, opens its page in a sheet inside the app that hands back when done.
 */
function useProviderSignIn() {
  const { startSSOFlow } = useSSO();
  const { startAppleAuthenticationFlow } = useSignInWithApple();
  const { startGoogleAuthenticationFlow } = useSignInWithGoogle();
  const [busy, setBusy] = useState<Provider>();
  const [problem, setProblem] = useState<string>();
  const start = async (strategy: Provider) => {
    if (busy) return;
    setBusy(strategy);
    setProblem(undefined);
    try {
      const { createdSessionId, setActive } =
        strategy === 'oauth_apple'
          ? await startAppleAuthenticationFlow()
          : strategy === 'oauth_google' && NATIVE_GOOGLE
            ? await startGoogleAuthenticationFlow()
            : await startSSOFlow({ strategy, redirectUrl: AuthSession.makeRedirectUri({ path: 'sso-callback' }) });
      if (createdSessionId && setActive) await setActive({ session: createdSessionId });
    } catch (error) {
      setProblem(describe(error));
    } finally {
      setBusy(undefined);
    }
  };
  return { start, busy, problem };
}

export function Welcome() {
  const bottom = useBottomGap();
  const provider = useProviderSignIn();
  return (
    <SafeAreaView style={styles.screen} edges={['top']}>
      <Reel />
      <View style={styles.copy}>
        <View style={styles.markLine}>
          <Icon name="Logo" size={22} color={colors.ink} />
          <Text style={styles.markText}>Sikemux</Text>
        </View>
        <Text style={styles.title}>Your agents,{'\n'}on your phone.</Text>
      </View>
      <View style={[styles.actions, { paddingBottom: bottom }]}>
        {APPLE ? (
          <Button
            title="Continue with Apple"
            icon={<AppleMark color={colors.ink} />}
            busy={provider.busy === 'oauth_apple'}
            onPress={() => provider.start('oauth_apple')}
            style={styles.provider}
          />
        ) : null}
        <Button
          title="Continue with Google"
          icon={<GoogleG />}
          busy={provider.busy === 'oauth_google'}
          onPress={() => provider.start('oauth_google')}
          style={styles.provider}
        />
        <Button
          title="Continue with GitHub"
          icon={<GitHubMark color={colors.ink} />}
          busy={provider.busy === 'oauth_github'}
          onPress={() => provider.start('oauth_github')}
          style={styles.provider}
        />
        <Pressable onPress={() => router.push('/sign-in')} style={styles.paste} accessibilityRole="button">
          <Text style={styles.pasteText}>Continue with email</Text>
        </Pressable>
        {provider.problem ? <Text style={styles.problem}>{provider.problem}</Text> : null}
      </View>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.ground },
  reel: { height: REEL_HEIGHT, flexShrink: 1, overflow: 'hidden', paddingHorizontal: 16, marginTop: 8 },
  fade: { position: 'absolute', left: 0, right: 0, height: 110 },
  row: { height: ROW, flexDirection: 'row', alignItems: 'center', gap: 12, paddingHorizontal: 10 },
  rowTitle: { color: colors.ink },
  rowPlace: { flexDirection: 'row', alignItems: 'center', gap: 4, marginTop: 1 },
  rowDetail: { ...type.meta, fontSize: 12.5 },
  state: { width: 20, alignItems: 'center' },
  copy: { flexGrow: 1, justifyContent: 'flex-end', paddingHorizontal: 24 },
  markLine: { flexDirection: 'row', alignItems: 'center', gap: 9 },
  markText: { fontFamily: fonts.uiSemibold, fontSize: 15, color: colors.ink, letterSpacing: -0.2 },
  title: { marginTop: 18, fontFamily: fonts.uiSemibold, fontSize: 34, lineHeight: 37, letterSpacing: -1.2, color: colors.ink },
  actions: { paddingHorizontal: 16, paddingTop: 28, gap: 12 },
  provider: { backgroundColor: colors.raised },
  problem: { ...type.meta, color: colors.danger, textAlign: 'center' },
  paste: { height: 44, alignItems: 'center', justifyContent: 'center' },
  pasteText: { fontFamily: fonts.uiMedium, fontSize: 15, color: colors.secondary },
});
