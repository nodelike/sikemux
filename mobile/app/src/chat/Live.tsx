import { Fragment, useEffect, useState, type ReactNode } from 'react';
import { ActivityIndicator, Animated, Pressable, StyleSheet, Text, View } from 'react-native';

import {
  subagentActivity,
  subagentCost,
  taskDetail,
  workflowAgentCost,
  workflowAgentCount,
  workflowAgentDetail,
} from '@mac/chat/transcript';
import type { WorkflowAgent } from '@mac/chat/types';
import { queuedLabel } from '@mac/chat/queuedMessages';
import { AgentIcon, Icon } from '@/ui/Icon';
import type { IconName } from '@/ui/icons.generated';
import { SectionLabel } from '@/ui/list';
import { useStill } from '@/ui/motion';
import { Sheet } from '@/ui/Sheet';
import { Shimmer } from '@/ui/Shimmer';
import { Dot, Working } from '@/ui/status';
import { fonts, mixed, type Palette, useColors, useStyles } from '@/ui/theme';
import { counted } from '@/devices/words';
import { liveStrip, type LiveWork, type PlanEntry, type StripPiece } from './liveWork';

const TASK_ICONS: Readonly<Record<string, IconName>> = { shell: 'IconCommand', workflow: 'IconWorkflow' };

function TaskIcon({ kind, color, size = 12 }: { kind: string; color: string; size?: number }) {
  return <Icon name={TASK_ICONS[kind] ?? 'IconTimer'} size={size} color={color} />;
}

/** Past this many kinds the strip drops the words and keeps the counts. */
const WORDED = 3;

/**
 * One line docked on the composer saying what is still going: subagents, background tasks by
 * kind, waiting messages and the plan's progress. A tap opens all of it.
 */
/** How long the strip shows one live thing before it turns to the next. */
const TURN_MS = 3200;

/** Which of `count` live lines the strip shows, turning to the next with a short fade. */
function useTurns(count: number): { index: number; shown: Animated.Value } {
  const still = useStill();
  const [index, setIndex] = useState(0);
  const [shown] = useState(() => new Animated.Value(1));
  useEffect(() => {
    if (count < 2) return;
    const timer = setInterval(() => {
      if (still) {
        setIndex((current) => current + 1);
        return;
      }
      Animated.timing(shown, { toValue: 0, duration: 140, useNativeDriver: true }).start(() => {
        setIndex((current) => current + 1);
        Animated.timing(shown, { toValue: 1, duration: 140, useNativeDriver: true }).start();
      });
    }, TURN_MS);
    return () => clearInterval(timer);
  }, [count, still, shown]);
  return { index, shown };
}

function PieceIcon({ piece, provider }: { piece: StripPiece; provider: string }) {
  const colors = useColors();
  if (piece.task) return <TaskIcon kind={piece.task} color={colors.inkDim} />;
  if (piece.key === 'subagents') return <AgentIcon provider={provider} size={12} />;
  return <Icon name={piece.key === 'queued' ? 'IconClock' : 'IconCheck'} size={12} color={colors.inkDim} />;
}

export function LiveStrip({ work, provider, onOpen }: { work: LiveWork; provider: string; onOpen: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const { pieces, lines, working } = liveStrip(work);
  const line = useTurns(lines.length);
  const worded = !lines.length && pieces.length <= WORDED;
  return (
    <Pressable
      onPress={onOpen}
      style={({ pressed }) => [styles.strip, pressed && { backgroundColor: colors.raised }]}
      accessibilityRole="button"
      accessibilityLabel={`${pieces.map((piece) => `${piece.count} ${piece.word}`).join(', ')}. Show`}>
      {working ? <Working /> : null}
      {lines.length ? (
        <Animated.View style={[styles.live, { opacity: line.shown }]}>
          <Shimmer style={styles.liveText} layout={styles.liveLayout}>
            {lines[line.index % lines.length]}
          </Shimmer>
        </Animated.View>
      ) : null}
      <View style={[styles.pieces, lines.length > 0 && styles.piecesAfter]}>
        {pieces.map((piece) => (
          <View key={piece.key} style={styles.piece}>
            <PieceIcon piece={piece} provider={provider} />
            <Text style={styles.pieceText} numberOfLines={1}>
              {piece.key === 'plan' && worded ? `${piece.word} ` : null}
              <Text style={styles.count}>{piece.count}</Text>
              {piece.key !== 'plan' && worded ? ` ${piece.word}` : null}
            </Text>
          </View>
        ))}
      </View>
      <View style={styles.up}>
        <Icon name="IconChevron" size={10} color={colors.inkFaint} />
      </View>
    </Pressable>
  );
}

function SmallButton({
  label,
  onPress,
  busy,
  accessibilityLabel,
}: {
  label: string;
  onPress: () => void;
  busy?: boolean;
  accessibilityLabel: string;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <Pressable
      onPress={onPress}
      disabled={busy}
      hitSlop={6}
      style={({ pressed }) => [styles.small, pressed && { backgroundColor: colors.active }]}
      accessibilityRole="button"
      accessibilityLabel={accessibilityLabel}
      accessibilityState={{ busy, disabled: busy }}>
      {busy ? <ActivityIndicator size="small" color={colors.inkDim} /> : <Text style={styles.smallText}>{label}</Text>}
    </Pressable>
  );
}

function Line({
  mark,
  title,
  detail,
  cost,
  end,
  faint,
  nested,
}: {
  mark: ReactNode;
  title: string;
  detail?: string;
  /** What a subagent has spent so far, or how many of a workflow's agents are done. */
  cost?: string;
  end?: ReactNode;
  faint?: boolean;
  /** One of a workflow's agents, set in under the workflow. */
  nested?: boolean;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={[styles.line, nested && styles.nested]}>
      <View style={[styles.mark, faint && styles.faded]}>{mark}</View>
      <View style={styles.lineBody}>
        <Text style={[styles.lineTitle, faint && { color: colors.tertiary }]} numberOfLines={1}>
          {title}
        </Text>
        {detail || cost ? (
          <View style={styles.lineDetailRow}>
            {detail ? (
              <Text style={styles.lineDetail} numberOfLines={1}>
                {detail}
              </Text>
            ) : null}
            {cost ? <Text style={styles.cost}>{cost}</Text> : null}
          </View>
        ) : null}
      </View>
      {end ? <View style={styles.lineEnd}>{end}</View> : null}
    </View>
  );
}

function WorkflowAgentMark({ state }: { state: WorkflowAgent['state'] }) {
  const colors = useColors();
  if (state === 'running') return <Working />;
  if (state === 'completed') return <Icon name="IconCheck" size={12} color={mixed(colors.live, colors.inkFaint, 0.58)} />;
  if (state === 'failed') return <Icon name="IconClose" size={11} color={colors.danger} />;
  return null;
}

function PlanMark({ status }: { status: PlanEntry['status'] }) {
  const colors = useColors();
  if (status === 'completed') return <Icon name="IconCheck" size={14} color={colors.inkDim} />;
  if (status === 'in_progress') return <Working />;
  return <Dot color={colors.rest} size={8} hollow />;
}

/** Everything the strip sums up, in the Mac's live stack's order, with what can be done about each. */
export function LiveSheet({
  visible,
  onClose,
  work,
  provider,
  steerable,
  canAct,
  stopping,
  onStopTask,
  onSteer,
  onDrop,
}: {
  visible: boolean;
  onClose: () => void;
  work: LiveWork;
  provider: string;
  /** The agent takes a message mid-turn, and a turn is running. */
  steerable: boolean;
  /** This phone may act on the chat rather than only watch it. */
  canAct: boolean;
  stopping: ReadonlySet<string>;
  onStopTask: (taskId: string) => void;
  onSteer: (ids: string[]) => void;
  onDrop: (id: string) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const done = work.plan.filter((entry) => entry.status === 'completed').length;
  return (
    <Sheet visible={visible} onClose={onClose}>
      <View style={styles.sheet}>
        {work.subagents.length ? (
          <>
            <SectionLabel count={work.subagents.length}>{counted('subagent', work.subagents.length)}</SectionLabel>
            {work.subagents.map((subagent) => (
              <Line
                key={subagent.sessionId}
                mark={<AgentIcon provider={provider} size={16} />}
                title={subagent.name}
                detail={subagentActivity(subagent)}
                cost={subagentCost(subagent)}
                end={<Working />}
              />
            ))}
          </>
        ) : null}
        {work.tasks.map(([kind, tasks]) => (
          <Fragment key={kind}>
            <SectionLabel count={tasks.length}>{counted(kind, tasks.length)}</SectionLabel>
            {tasks.map((task) => (
              <Fragment key={task.asyncTaskId}>
                <Line
                  mark={<TaskIcon kind={kind} size={15} color={task.state === 'paused' ? colors.warn : colors.inkDim} />}
                  title={task.name}
                  detail={task.state === 'paused' ? 'paused' : taskDetail(task)}
                  cost={task.agents?.length ? workflowAgentCount(task.agents) : undefined}
                  end={
                    canAct && task.canStop ? (
                      <SmallButton
                        label="Stop"
                        busy={stopping.has(task.asyncTaskId)}
                        onPress={() => onStopTask(task.asyncTaskId)}
                        accessibilityLabel={`Stop ${task.name}`}
                      />
                    ) : null
                  }
                />
                {task.agents?.map((agent) => (
                  <Line
                    key={agent.index}
                    nested
                    faint={agent.state === 'queued' || agent.state === 'completed'}
                    mark={<AgentIcon provider={provider} size={16} />}
                    title={agent.label}
                    detail={workflowAgentDetail(agent)}
                    cost={workflowAgentCost(agent)}
                    end={<WorkflowAgentMark state={agent.state} />}
                  />
                ))}
              </Fragment>
            ))}
          </Fragment>
        ))}
        {work.queued.length ? (
          <>
            <SectionLabel
              count={work.queued.length}
              action={
                steerable && work.queued.length > 1 ? (
                  <SmallButton
                    label="Steer all"
                    onPress={() => onSteer(work.queued.map((held) => held.id))}
                    accessibilityLabel="Steer the running turn with every queued message"
                  />
                ) : null
              }>
              queued
            </SectionLabel>
            {work.queued.map((held) => {
              const files = held.paths.length;
              const label = queuedLabel(held);
              return (
                <Line
                  key={held.id}
                  mark={<Icon name="IconClock" size={15} color={colors.inkDim} />}
                  title={label}
                  detail={held.text && files ? `${files} ${counted('file', files)}` : undefined}
                  end={
                    <>
                      {steerable ? (
                        <SmallButton
                          label="Steer"
                          onPress={() => onSteer([held.id])}
                          accessibilityLabel={`Steer the running turn with ${label}`}
                        />
                      ) : null}
                      <Pressable
                        onPress={() => onDrop(held.id)}
                        hitSlop={8}
                        style={({ pressed }) => [styles.drop, pressed && { backgroundColor: colors.active }]}
                        accessibilityRole="button"
                        accessibilityLabel={`Drop ${label} from the queue`}>
                        <Icon name="IconClose" size={11} color={colors.inkDim} />
                      </Pressable>
                    </>
                  }
                />
              );
            })}
          </>
        ) : null}
        {work.plan.length ? (
          <>
            <SectionLabel action={<Text style={styles.progress}>{`${done} of ${work.plan.length}`}</Text>}>plan</SectionLabel>
            {work.plan.map((entry, index) => (
              <View key={index} style={styles.step}>
                <View style={styles.stepMark}>
                  <PlanMark status={entry.status} />
                </View>
                <Text
                  style={[
                    styles.stepText,
                    entry.status === 'completed' && { color: colors.tertiary },
                    entry.status === 'in_progress' && { color: colors.ink },
                  ]}>
                  {entry.content}
                </Text>
              </View>
            ))}
          </>
        ) : null}
      </View>
    </Sheet>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    strip: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      height: 34,
      marginHorizontal: 14,
      marginBottom: -1,
      paddingLeft: 12,
      paddingRight: 10,
      borderWidth: 1,
      borderBottomWidth: 0,
      borderColor: colors.border,
      borderTopLeftRadius: 12,
      borderTopRightRadius: 12,
      backgroundColor: colors.composer,
    },
    pieces: { flex: 1, flexDirection: 'row', alignItems: 'center', gap: 12, overflow: 'hidden' },
    piecesAfter: { flex: 0, gap: 10 },
    live: { flex: 1, minWidth: 0 },
    liveLayout: { flexShrink: 1 },
    liveText: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.secondary },
    piece: { flexDirection: 'row', alignItems: 'center', gap: 5, flexShrink: 1 },
    pieceText: { fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
    count: { fontFamily: fonts.uiMedium, color: colors.ink, fontVariant: ['tabular-nums'] },
    up: { transform: [{ rotate: '-90deg' }] },
    sheet: { paddingBottom: 8 },
    line: { flexDirection: 'row', alignItems: 'center', gap: 10, minHeight: 48, paddingHorizontal: 8, paddingVertical: 6 },
    mark: { width: 20, height: 20, alignItems: 'center', justifyContent: 'center' },
    lineBody: { flex: 1, minWidth: 0 },
    lineTitle: { fontFamily: fonts.uiMedium, fontSize: 14.5, letterSpacing: -0.15, color: colors.secondary },
    lineDetailRow: { flexDirection: 'row', alignItems: 'baseline', gap: 8, marginTop: 1 },
    lineDetail: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
    cost: { fontFamily: fonts.mono, fontSize: 10.5, color: colors.inkFaint, fontVariant: ['tabular-nums'] },
    nested: { paddingLeft: 38 },
    faded: { opacity: 0.6 },
    lineEnd: { flexDirection: 'row', alignItems: 'center', gap: 8 },
    small: {
      minWidth: 52,
      height: 28,
      paddingHorizontal: 10,
      borderRadius: 8,
      borderWidth: 1,
      borderColor: colors.borderStrong,
      alignItems: 'center',
      justifyContent: 'center',
    },
    smallText: { fontFamily: fonts.uiMedium, fontSize: 12.5, color: colors.ink },
    drop: { width: 28, height: 28, borderRadius: 8, alignItems: 'center', justifyContent: 'center' },
    progress: { fontFamily: fonts.ui, fontSize: 11, color: colors.inkDim, fontVariant: ['tabular-nums'] },
    step: { flexDirection: 'row', alignItems: 'flex-start', gap: 10, paddingHorizontal: 8, paddingVertical: 7 },
    stepMark: { width: 20, height: 20, alignItems: 'center', justifyContent: 'center' },
    stepText: { flex: 1, fontFamily: fonts.ui, fontSize: 14, lineHeight: 20, color: colors.secondary },
  });
};
