import { useEffect, useState } from 'react';
import { Animated, Easing, StyleSheet, Text, View } from 'react-native';

import Svg, { Circle } from 'react-native-svg';

import type { AgentGroup, TurnState } from '@shared/wire.generated';
import { ringShare, rowLabel, rowMinutes } from '@shared/calls/words';
import { useT } from '@/i18n';
import { counted } from './words';

import { Icon } from '@/ui/Icon';
import { useAnimating } from '@/ui/motion';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

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

/** How soon a snoozed agent wakes, as the Mac rail says it: minutes, hours, then the weekday. */
export function WakeTime({ at, now }: { at: number; now: number }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const left = Math.max(0, at - now);
  const hour = 3_600_000;
  const words =
    left < hour
      ? `${Math.max(1, Math.ceil(left / 60_000))}m`
      : left < 24 * hour
        ? `${Math.max(1, Math.round(left / hour))}h`
        : new Date(at).toLocaleDateString(undefined, { weekday: 'short' });
  return (
    <View style={styles.count} accessible accessibilityLabel={`Snoozed, wakes in ${words}`}>
      <Icon name="IconClock" size={11} color={colors.tertiary} />
      <Text style={[styles.countText, { color: colors.tertiary }]}>{words}</Text>
    </View>
  );
}

/**
 * How an agent's turn stands, as the Mac's rail and the notch mark a row, one state per row: its working squares; a
 * ring filling in the neutral ink as a CI or check wait with an estimate runs, with the minutes left; the pink timer for
 * any other wait, with the time left when it waits for a set time; the amber dot with how long a call has waited; the
 * purple dot once done, red once failed, a hollow ring once stopped. The core says which.
 */
export function TurnMark({ turn, now }: { turn?: TurnState; now: number }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  if (!turn) return null;
  const share = turn.state === 'waiting' ? ringShare(turn.wait, now) : undefined;
  const time = rowMinutes(turn, now);
  const tint = turn.state === 'needsYou' ? colors.warn : turn.state === 'waiting' && share == null ? colors.cmd : colors.tertiary;
  return (
    <View style={styles.count} accessible accessibilityLabel={rowLabel(turn, now, t)}>
      {time ? <Text style={[styles.time, { color: tint }]}>{time}</Text> : null}
      <View style={styles.markBox}>
        {turn.state === 'working' ? (
          <Working />
        ) : turn.state === 'waiting' ? (
          share != null ? (
            <WaitRing share={share} />
          ) : (
            <Icon name="IconTimer" size={11} color={colors.cmd} />
          )
        ) : turn.state === 'needsYou' ? (
          <Dot color={colors.warn} size={7} />
        ) : turn.state === 'done' ? (
          <Dot color={colors.accent} size={7} />
        ) : turn.state === 'failed' ? (
          <Dot color={colors.danger} size={7} />
        ) : turn.state === 'stopped' ? (
          <Dot color={colors.secondary} size={7} hollow />
        ) : null}
      </View>
    </View>
  );
}

/** A ring that fills clockwise from the top in the neutral ink as a wait runs, over the border's track. */
export function WaitRing({ share }: { share: number }) {
  const colors = useColors();
  const radius = (11 - 1.6) / 2;
  const around = 2 * Math.PI * radius;
  return (
    <Svg width={11} height={11}>
      <Circle cx={5.5} cy={5.5} r={radius} stroke={colors.border} strokeWidth={1.6} fill="none" />
      <Circle
        cx={5.5}
        cy={5.5}
        r={radius}
        stroke={colors.secondary}
        strokeWidth={1.6}
        strokeLinecap="round"
        fill="none"
        strokeDasharray={`${around * share} ${around}`}
        transform="rotate(-90 5.5 5.5)"
      />
    </Svg>
  );
}

/** A linked host's worker, whose row carries no turn yet: squares while it works, a dot once something waits. */
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
    time: { fontFamily: fonts.ui, fontSize: 11, fontVariant: ['tabular-nums'] },
    markBox: { width: 16, height: 16, alignItems: 'center', justifyContent: 'center' },
  });
};
