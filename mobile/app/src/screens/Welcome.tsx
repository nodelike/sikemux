import { useEffect, useState } from 'react';
import { AccessibilityInfo, Animated, Easing, Pressable, StyleSheet, Text, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { router } from 'expo-router';
import Svg, { Defs, LinearGradient, Rect, Stop } from 'react-native-svg';

import { pasteFoundLink } from '@/devices/foundLinks';
import { AgentIcon, Icon } from '@/ui/Icon';
import { Button, Dot, NeedsYou, useBottomGap, Working } from '@/ui/parts';
import { defaultPalette as colors, fonts, typeFor } from '@/ui/theme';

// Shown before any Mac is paired, so it is drawn in the default theme.
const type = typeFor(colors);

/** What the app is for, before there is anything of the person's to show: the rail, drifting past. */
const REEL = [
  { provider: 'claude', title: 'Tighten the rail spacing', detail: 'sikemux · Editing…', state: 'working' },
  { provider: 'codex', title: 'Fix the flaky PTY replay test', detail: 'sikemux · Running a command…', state: 'working' },
  { provider: 'claude', title: 'Remote pairing screen', detail: 'sikemux · Needs input', state: 'needs' },
  { provider: 'hermes', title: 'Release notes for v0.4.3', detail: 'sikemux-front · Done', state: 'done' },
  { provider: 'opencode', title: 'Why does the shader dim the rail', detail: 'sikemux · Thinking…', state: 'working' },
  { provider: 'codex', title: 'Download size on the hero', detail: 'sikemux-front · Reading…', state: 'working' },
] as const;

const ROW = 56;
const REEL_HEIGHT = 340;

function ReelRow({ row }: { row: (typeof REEL)[number] }) {
  return (
    <View style={styles.row}>
      <AgentIcon provider={row.provider} size={22} />
      <View style={{ flex: 1 }}>
        <Text style={type.row} numberOfLines={1}>
          {row.title}
        </Text>
        <Text style={styles.rowDetail}>{row.detail}</Text>
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

export function Welcome() {
  const bottom = useBottomGap();
  return (
    <SafeAreaView style={styles.screen} edges={['top']}>
      <Reel />
      <View style={styles.copy}>
        <View style={styles.markLine}>
          <Icon name="Logo" size={22} color={colors.ink} />
          <Text style={styles.markText}>Sikemux</Text>
        </View>
        <Text style={styles.title}>Your agents,{'\n'}on your phone.</Text>
        <Text style={styles.body}>Watch them work, answer what they ask, and open the Mac&apos;s terminals.</Text>
      </View>
      <View style={[styles.actions, { paddingBottom: bottom }]}>
        <Button kind="primary" title="Scan the code on your Mac" onPress={() => router.push('/scan')} />
        <Pressable onPress={() => pasteFoundLink()} style={styles.paste} accessibilityRole="button">
          <Text style={styles.pasteText}>Paste a pairing link</Text>
        </Pressable>
      </View>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.ground },
  reel: { height: REEL_HEIGHT, overflow: 'hidden', paddingHorizontal: 16, marginTop: 8 },
  fade: { position: 'absolute', left: 0, right: 0, height: 110 },
  row: { height: ROW, flexDirection: 'row', alignItems: 'center', gap: 12, paddingHorizontal: 10 },
  rowDetail: { ...type.meta, fontSize: 12.5, marginTop: 1 },
  state: { width: 20, alignItems: 'center' },
  copy: { flex: 1, justifyContent: 'flex-end', paddingHorizontal: 24 },
  markLine: { flexDirection: 'row', alignItems: 'center', gap: 9 },
  markText: { fontFamily: fonts.uiSemibold, fontSize: 15, color: colors.ink, letterSpacing: -0.2 },
  title: { marginTop: 18, fontFamily: fonts.uiSemibold, fontSize: 34, lineHeight: 37, letterSpacing: -1.2, color: colors.ink },
  body: { ...type.body, fontSize: 16, lineHeight: 24, marginTop: 12, maxWidth: 300 },
  actions: { paddingHorizontal: 16, paddingTop: 28, gap: 4 },
  paste: { height: 44, alignItems: 'center', justifyContent: 'center' },
  pasteText: { fontFamily: fonts.uiMedium, fontSize: 15, color: colors.secondary },
});
