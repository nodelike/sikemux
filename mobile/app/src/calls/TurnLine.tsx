import { useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';

import { agentLabel } from '@shared/agents/catalog';
import { docked, LOOK, said, stepLabel, type Said } from '@shared/calls/words';
import type { TurnPhase, TurnState, TurnStep } from '@shared/wire.generated';
import type { ConnectionLike } from '@sikemux/native';
import { callHost } from '@/core/hostCalls';
import { problem } from '@/core/problem';
import { Dot, Working } from '@/devices/AgentMarks';
import { useT } from '@/i18n';
import { Chip } from '@/ui/controls';
import { Icon } from '@/ui/Icon';
import { useAnimating, useMinute, useSecond } from '@/ui/motion';
import { Shimmer } from '@/ui/Shimmer';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/**
 * The chat's last line once a turn has ended, or while it works on a step it named: the state's mark, how the turn
 * stands in the core's words, and the next steps as buttons on the same row. A failed turn's line is the trouble
 * card's. With the call it waits on docked right under it, the line says only how many wait and how long.
 */
export function TurnLine({
  agentId,
  provider,
  turn,
  connection,
  canAct,
  docked: callDocked,
  onLook,
}: {
  agentId: string;
  provider: string;
  turn: TurnState;
  connection?: ConnectionLike;
  canAct: boolean;
  /** The call the turn waits on is docked on this chat. */
  docked: boolean;
  onLook: () => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const animating = useAnimating();
  // Only a working turn counts seconds; the other states show minutes, on the same minute as the call card's age.
  const ticks = turn.state === 'working';
  const second = useSecond(ticks && animating);
  const minute = useMinute();
  const now = ticks && second ? second * 1000 : minute * 60_000;
  const [taking, setTaking] = useState<TurnStep>();
  const [refused, setRefused] = useState<string>();
  if (turn.state === 'failed') return null;
  const line = said(turn, agentLabel(provider), now, t);
  if (!line) return null;
  const shown: Said = callDocked ? docked(line, turn.call?.count ?? 1, t) : line;

  /** Takes a step; a production step raises its call, which docks on this chat to be answered. */
  const take = async (step: TurnStep) => {
    if (taking || !connection) return;
    setTaking(step);
    setRefused(undefined);
    try {
      await callHost(connection, 'takeStep', { agentId, step });
    } catch (error) {
      setRefused(problem(error));
    } finally {
      setTaking(undefined);
    }
  };
  const chip = (step: TurnStep, primary: boolean) => {
    const label = stepLabel(step, t);
    return label ? <Chip key={step} title={label} primary={primary} busy={taking === step} onPress={() => void take(step)} /> : null;
  };
  const buttons = !canAct ? null : turn.state === 'done' ? (
    (turn.done?.next ?? []).map((next) => chip(next.step, next.primary))
  ) : turn.state === 'needsYou' && !callDocked ? (
    <Chip title={t(LOOK)} primary onPress={onLook} />
  ) : turn.state === 'stopped' ? (
    chip('carryOn', false)
  ) : null;

  // A wait's place, holder and minutes matter more than its long name, which gives way first.
  const waiting = shown.phase === 'waiting';
  return (
    <View style={styles.wrap}>
      <View style={styles.line}>
        <View style={styles.mark}>
          <Mark phase={turn.state} />
        </View>
        <View style={styles.words} accessible accessibilityLabel={[shown.word, ...shown.parts].join(', ')}>
          {shown.phase === 'working' ? (
            <Shimmer style={[styles.word, { color: colors.ink }]}>{shown.word}</Shimmer>
          ) : (
            <Text
              style={[styles.word, waiting && styles.shortened, { color: tint(shown.phase, colors) }]}
              numberOfLines={waiting ? 1 : undefined}>
              {shown.word}
            </Text>
          )}
          {shown.parts.map((part, index) => (
            <View key={index} style={[styles.part, waiting && styles.whole]}>
              <Text style={styles.dot}>·</Text>
              <Text style={[styles.partText, { color: index === 0 ? colors.secondary : colors.tertiary }]} numberOfLines={1}>
                {part}
              </Text>
            </View>
          ))}
        </View>
        {buttons ? <View style={styles.buttons}>{buttons}</View> : null}
      </View>
      {refused ? <Text style={styles.refused}>{refused}</Text> : null}
    </View>
  );
}

/** The state's mark, one set everywhere. */
function Mark({ phase }: { phase: TurnPhase }) {
  const colors = useColors();
  switch (phase) {
    case 'working':
      return <Working />;
    case 'waiting':
      return <Icon name="IconTimer" size={14} color={colors.cmd} />;
    case 'needsYou':
      return <Dot color={colors.warn} size={8} />;
    case 'done':
      return <Dot color={colors.accent} size={8} />;
    case 'stopped':
      return <Dot color={colors.secondary} size={8} hollow />;
    default:
      return null;
  }
}

function tint(phase: TurnPhase, colors: Palette): string {
  switch (phase) {
    case 'done':
      return colors.accent;
    case 'waiting':
      return colors.cmd;
    case 'needsYou':
      return colors.warn;
    case 'failed':
      return colors.danger;
    case 'stopped':
      return colors.secondary;
    default:
      return colors.ink;
  }
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    wrap: { marginTop: 10, gap: 4 },
    line: { flexDirection: 'row', alignItems: 'center', gap: 8, minHeight: 32 },
    mark: { width: 16, height: 16, alignItems: 'center', justifyContent: 'center' },
    words: { flex: 1, minWidth: 0, flexDirection: 'row', alignItems: 'center', gap: 6 },
    word: { fontFamily: fonts.uiMedium, fontSize: 13 },
    shortened: { flexShrink: 1 },
    whole: { flexShrink: 0 },
    part: { flexDirection: 'row', alignItems: 'center', gap: 6, flexShrink: 1, minWidth: 0 },
    dot: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkFaint },
    partText: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 12.5 },
    buttons: { flexDirection: 'row', alignItems: 'center', gap: 6 },
    refused: { marginLeft: 24, fontFamily: fonts.ui, fontSize: 12.5, color: colors.danger },
  });
};
