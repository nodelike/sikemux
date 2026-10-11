import { useEffect, useState } from 'react';
import { Animated, Easing, StyleSheet, Text, View } from 'react-native';

import { Icon } from './Icon';
import { useAnimating } from './motion';
import { fonts, type Palette, translucent, useColors, useStyles } from './theme';

function useLoop(still: boolean, duration: number, delay = 0) {
  const [value] = useState(() => new Animated.Value(0));
  useEffect(() => {
    if (still) {
      value.setValue(0);
      return;
    }
    // A delay inside a native-driven loop stops the loop after its first run, so the offset is waited once.
    const loop = Animated.sequence([
      Animated.delay(delay),
      Animated.loop(Animated.timing(value, { toValue: 1, duration, easing: Easing.inOut(Easing.ease), useNativeDriver: true })),
    ]);
    loop.start();
    return () => loop.stop();
  }, [value, duration, delay, still]);
  return value;
}

const TWINKLE = [1200, 1580, 900, 1400, 1050, 1300, 950, 1500, 1150];

function TwinkleCell({ period, index, still }: { period: number; index: number; still: boolean }) {
  const styles = useStyles(makeStyles);
  const phase = useLoop(still, period, (index * 230) % period);
  const opacity = phase.interpolate({ inputRange: [0, 0.5, 1], outputRange: [0.2, 1, 0.2] });
  return <Animated.View style={[styles.cell, { opacity }]} />;
}

/** An agent at work: the rail's three-by-three twinkling squares. */
export function Working() {
  const styles = useStyles(makeStyles);
  const animating = useAnimating();
  return (
    <View style={styles.loader} accessible accessibilityLabel="Working">
      {TWINKLE.map((period, index) => (
        <TwinkleCell key={index} period={period} index={index} still={!animating} />
      ))}
    </View>
  );
}

/** Something is waiting on the person: a white dot sending out a ring. */
export function NeedsYou() {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const animating = useAnimating();
  const phase = useLoop(!animating, 2000);
  const scale = phase.interpolate({ inputRange: [0, 1], outputRange: [0.6, 1.8] });
  const opacity = phase.interpolate({ inputRange: [0, 1], outputRange: [0.9, 0] });
  return (
    <View style={styles.dotBox} accessible accessibilityLabel="Needs input">
      <Animated.View style={[styles.ring, { transform: [{ scale }], opacity }]} />
      <View style={[styles.dot, { backgroundColor: colors.ink }]} />
    </View>
  );
}

/** Shells or monitors an agent left running, in the Mac's terminal glyph so it does not read as the dot that waits on you. */
export function StillRunning() {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.dotBox} accessible accessibilityLabel="Shells or monitors still running">
      <Icon name="IconCommand" size={11} color={colors.warn} />
    </View>
  );
}

/** The subagents an agent still has running, as the Mac's tabs and rail count them. */
export function SubagentCount({ count }: { count: number }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.subagents} accessible accessibilityLabel={`${count} ${count === 1 ? 'subagent' : 'subagents'} running`}>
      <Icon name="IconAgent" size={11} color={translucent(colors.live, 0.75)} filled />
      <Text style={styles.subagentCount}>{count}</Text>
    </View>
  );
}

export function Dot({ color, size = 8, hollow }: { color: string; size?: number; hollow?: boolean }) {
  return (
    <View
      style={[
        { width: size, height: size, borderRadius: size },
        hollow ? { borderWidth: 1.5, borderColor: color } : { backgroundColor: color },
      ]}
    />
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    loader: { width: 13, height: 13, flexDirection: 'row', flexWrap: 'wrap', gap: 2 },
    cell: { width: 3, height: 3, borderRadius: 0.6, backgroundColor: colors.live },
    dotBox: { width: 16, height: 16, alignItems: 'center', justifyContent: 'center' },
    ring: { position: 'absolute', width: 16, height: 16, borderRadius: 8, borderWidth: 1.5, borderColor: colors.ink },
    dot: { width: 8, height: 8, borderRadius: 4 },
    subagents: { flexDirection: 'row', alignItems: 'center', gap: 3 },
    subagentCount: { fontFamily: fonts.uiMedium, fontSize: 12, color: colors.tertiary, fontVariant: ['tabular-nums'] },
  });
};
