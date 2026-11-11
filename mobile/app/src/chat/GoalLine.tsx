import { StyleSheet, Text, View } from 'react-native';

import { durationLabel } from '@shared/chat/durationLabel';
import type { AgentNotice, GoalMarker, SliceState } from '@shared/wire.generated';
import { fonts, type Palette, translucent, useColors, useStyles } from '@/ui/theme';

const WORDS: Record<SliceState, string> = {
  moving: 'Goal',
  done: 'Goal done',
  failed: 'Goal stopped',
  needsYou: 'Goal needs you',
  waiting: 'Goal waiting',
};

const STEP_WIDTH = 14;
const STEP_GAP = 3;
const STRIP_MOST = 96;

function stateInk(state: SliceState, colors: Palette): string {
  if (state === 'done' || state === 'moving') return colors.live;
  if (state === 'needsYou') return colors.warn;
  if (state === 'failed') return colors.danger;
  return colors.tertiary;
}

/** How far a goal's steps have come: "2 of 5 done", or of its gates. */
function progress(goal: GoalMarker): string | null {
  const steps = goal.steps ?? [];
  if (!steps.length) return null;
  const done = steps.filter((step) => step === 'done').length;
  return `${done} of ${steps.length} ${goal.ofGates ? 'gates' : 'done'}`;
}

/** Where a goal starts or ends, as a milestone: its mark and state in its colour, its name, then its steps and time. */
export function GoalLine({ notice, goal }: { notice: AgentNotice; goal: GoalMarker }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const words = progress(goal);
  const ink = stateInk(goal.state, colors);
  return (
    <View style={styles.line} accessibilityLabel={[WORDS[goal.state], goal.name || notice.title, words].filter(Boolean).join(', ')}>
      <StateMark state={goal.state} />
      <Text style={[styles.word, { color: ink }]}>{WORDS[goal.state]}</Text>
      <Text style={styles.name} numberOfLines={1}>
        {goal.name || notice.title}
      </Text>
      <View style={styles.end}>
        <GoalSteps steps={goal.steps ?? []} />
        {words ? <Text style={styles.progress}>{words}</Text> : null}
        {goal.took !== undefined && goal.took >= 1000 ? <Text style={styles.took}>{durationLabel(goal.took)}</Text> : null}
      </View>
    </View>
  );
}

/** A goal's steps as a strip of segments, one per step. */
export function GoalSteps({ steps }: { steps: readonly SliceState[] }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  if (!steps.length) return null;
  return (
    <View style={[styles.strip, { width: Math.min(STRIP_MOST, steps.length * STEP_WIDTH + (steps.length - 1) * STEP_GAP) }]}>
      {steps.map((step, index) => (
        <View key={index} style={[styles.step, segment(step, colors)]} />
      ))}
    </View>
  );
}

function segment(state: SliceState, colors: Palette) {
  if (state === 'done') return { backgroundColor: colors.live };
  if (state === 'moving') return { backgroundColor: translucent(colors.live, 0.45) };
  if (state === 'needsYou') return { backgroundColor: colors.warn };
  if (state === 'failed') return { backgroundColor: colors.danger };
  return { borderWidth: 1, borderStyle: 'dashed' as const, borderColor: colors.inkFaint };
}

/** A goal's mark: solid green done, a green ring moving, a dashed grey ring waiting, amber needing you, red stopped. */
export function StateMark({ state }: { state: SliceState }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const ink = stateInk(state, colors);
  const look =
    state === 'moving'
      ? { borderWidth: 1.5, borderColor: ink }
      : state === 'waiting'
        ? { borderWidth: 1, borderStyle: 'dashed' as const, borderColor: ink }
        : { backgroundColor: ink };
  return (
    <View style={styles.markBox}>
      <View style={[styles.dot, look]} />
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    line: { flexDirection: 'row', alignItems: 'center', gap: 9, minHeight: 36, marginVertical: 6 },
    markBox: { width: 14, height: 14, alignItems: 'center', justifyContent: 'center' },
    dot: { width: 8, height: 8, borderRadius: 4 },
    word: { flexShrink: 0, fontFamily: fonts.uiSemibold, fontSize: 12.5 },
    name: { flexShrink: 1, minWidth: 0, fontFamily: fonts.ui, fontSize: 12.5, color: colors.secondary },
    end: { marginLeft: 'auto', flexShrink: 0, flexDirection: 'row', alignItems: 'center', gap: 8 },
    strip: { flexDirection: 'row', gap: STEP_GAP },
    step: { flex: 1, height: 4, borderRadius: 2 },
    progress: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
    took: { fontFamily: fonts.ui, fontSize: 11.5, fontVariant: ['tabular-nums'], color: colors.inkFaint },
  });
};
