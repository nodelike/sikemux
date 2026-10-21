import { useEffect, useState } from 'react';
import { Animated, Easing, StyleSheet, Text, View } from 'react-native';

import type { AgentGroup } from '@/core/protocol';
import { counted } from '@/devices/words';

import { Icon } from './Icon';
import { useAnimating } from './motion';
import { fonts, type Palette, useColors, useStyles } from './theme';

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

/** Rings a needs-you dot sends out when it appears, as the Mac rail's and the notch's do, before it goes still. */
const PINGS = 2;

function Ping({ delay }: { delay: number }) {
  const styles = useStyles(makeStyles);
  const animating = useAnimating();
  const [phase] = useState(() => new Animated.Value(1));
  useEffect(() => {
    if (!animating) return;
    phase.setValue(0);
    const run = Animated.timing(phase, {
      toValue: 1,
      duration: 520,
      delay,
      easing: Easing.bezier(0.2, 0.6, 0.3, 1),
      useNativeDriver: true,
    });
    run.start();
    return () => run.stop();
  }, [phase, delay, animating]);
  const scale = phase.interpolate({ inputRange: [0, 1], outputRange: [0.6, 2.4] });
  const opacity = phase.interpolate({ inputRange: [0, 1], outputRange: [0.9, 0] });
  return <Animated.View style={[styles.ring, { transform: [{ scale }], opacity }]} />;
}

/** Something is waiting on the person: the Mac rail's amber dot, sending out two rings as it appears. */
export function NeedsYou() {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.dotBox} accessible accessibilityLabel="Needs input">
      {Array.from({ length: PINGS }, (_, ring) => (
        <Ping key={ring} delay={ring * 380} />
      ))}
      <View style={[styles.dot, { backgroundColor: colors.warn }]} />
    </View>
  );
}

/** The subagents an agent still has running, as the Mac's tabs and rail count them. */
export function SubagentCount({ count }: { count: number }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.count} accessible accessibilityLabel={`${count} ${counted('subagent', count)} running`}>
      <Icon name="IconAgent" size={11} color={colors.live} filled />
      <Text style={styles.countText}>{count}</Text>
    </View>
  );
}

/** The shells and monitors an agent still has running, counted beside the Mac's terminal glyph. */
export function ShellCount({ count }: { count: number }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View
      style={styles.count}
      accessible
      accessibilityLabel={`${count} ${count === 1 ? 'shell or monitor' : 'shells or monitors'} running`}>
      <Icon name="IconCommand" size={11} color={colors.warn} />
      <Text style={styles.countText}>{count}</Text>
    </View>
  );
}

/** Where an agent's turn is, as the Mac rail marks it: squares while it works, a dot once something waits, nothing idle. */
export function AgentState({ group }: { group: AgentGroup }) {
  const colors = useColors();
  if (group === 'working') return <Working />;
  if (group === 'blocked') return <NeedsYou />;
  if (group === 'done') return <Dot color={colors.accent} size={7} />;
  return null;
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
    ring: { position: 'absolute', width: 7, height: 7, borderRadius: 4, borderWidth: 1, borderColor: colors.warn },
    dot: { width: 7, height: 7, borderRadius: 4 },
    count: { flexDirection: 'row', alignItems: 'center', gap: 3 },
    countText: { fontFamily: fonts.uiMedium, fontSize: 12, color: colors.secondary, fontVariant: ['tabular-nums'] },
  });
};
