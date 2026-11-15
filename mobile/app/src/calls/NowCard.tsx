import { type ReactNode, useEffect, useState } from 'react';
import { Animated, Easing, Pressable, ScrollView, StyleSheet, Text, useWindowDimensions, View } from 'react-native';

import { inLine, NOT_CONNECTED } from '@shared/calls/words';
import { formatAge } from '@shared/lib/format';
import type { Say } from '@shared/lib/say';
import type { NowClosed, NowFact, NowGroup, NowRow, ProjectNow, WorkspaceAgent } from '@shared/wire.generated';
import type { ConnectionLike } from '@sikemux/native';
import { AgentFace } from '@/devices/Buddy';
import { Dot } from '@/devices/AgentMarks';
import { callHost } from '@/core/hostCalls';
import { problem } from '@/core/problem';
import { useT } from '@/i18n';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { useAnimating, useMinute } from '@/ui/motion';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/** Rows of a group shown before the rest fold into one line of marks. */
const SHOWN = 4;
const CLOSED_SHOWN = 3;
const LATE_MS = 10 * 60_000;

/**
 * The Now card over a chief's chat, as the Mac draws it: how many of its project's agents wait on the person, are
 * ready to look at, are moving and stand unsure, set large; then the rows the person acts on, the moving ones as
 * marks, and the agents closed lately with Reopen. Folded, it is the four counts alone at the top right.
 */
export function NowCard({
  now,
  agents,
  connection,
  full,
  onOpen,
}: {
  now: ProjectNow;
  agents: WorkspaceAgent[] | undefined;
  connection: ConnectionLike | undefined;
  full: boolean;
  onOpen: (agentId: string) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const minute = useMinute();
  const tall = useWindowDimensions().height;
  const [folded, setFolded] = useState(false);
  const rows = now.rows ?? [];
  const closed = now.closed ?? [];
  const counts = countsOf(rows);
  const agent = (id: string) => agents?.find((known) => known.id === id);
  const fold = () => {
    haptics.select();
    setFolded((was) => !was);
  };
  if (folded)
    return (
      <View style={styles.foldedPlace} pointerEvents="box-none">
        <Pressable onPress={fold} style={[styles.face, styles.capsule]} accessibilityRole="button" accessibilityLabel={t('Open Now')}>
          {counts.map(({ group, count }) => (
            <View key={group} style={[styles.pill, group === 'waitingOnYou' && count > 0 && styles.pillYou]}>
              <Dot color={tintOf(group, colors)} size={6} />
              <Text style={[styles.pillText, inkOf(group, count, colors, colors.secondary)]}>{count}</Text>
            </View>
          ))}
          <Chevron open={false} />
        </Pressable>
      </View>
    );
  const clock = (since: number, late = false) => (
    <Text style={[styles.clock, late && { color: colors.warn }]}>{formatAge(since, minute * 60_000, t)}</Text>
  );
  const factRow = (row: NowRow) => {
    const known = agent(row.agent);
    if (!known) return null;
    return (
      <Pressable
        key={row.agent}
        onPress={() => onOpen(row.agent)}
        style={({ pressed }) => [styles.row, pressed && { backgroundColor: colors.active }]}
        accessibilityRole="button">
        <AgentPill agent={known} />
        <Text style={styles.fact} numberOfLines={1}>
          {factWords(row.fact, t)}
        </Text>
        {row.group === 'notSure' ? <Dot color={colors.tertiary} size={7} hollow /> : null}
        {clock(row.since, row.group === 'waitingOnYou' && minute * 60_000 - row.since > LATE_MS)}
      </Pressable>
    );
  };
  const rest = (left: NowRow[]) => {
    const known = left.map((row) => agent(row.agent)).filter((found) => found !== undefined);
    if (!known.length) return null;
    return (
      <View key="rest" style={styles.row}>
        <Marks agents={known} />
        <Text style={styles.fact} numberOfLines={1}>
          {known.map(nameOf).join(', ')}
        </Text>
      </View>
    );
  };
  const listed = (group: NowGroup) => {
    const shown = shownOf(rows, group);
    if (!shown.length) return null;
    return (
      <View key={group}>
        <Label words={t(groupWords(group))} count={shown.length} />
        {shown.slice(0, SHOWN).map(factRow)}
        {rest(shown.slice(SHOWN))}
      </View>
    );
  };
  const moving = shownOf(rows, 'moving');
  return (
    <View style={styles.openPlace} pointerEvents="box-none">
      <View style={[styles.face, styles.card]}>
        <Pressable onPress={fold} style={styles.head} accessibilityRole="button" accessibilityLabel={t('Fold Now')}>
          <Icon name="Logo" size={13} color={colors.accent} />
          <Text style={styles.title}>{t('Now')}</Text>
          <Text style={styles.sub}>{rows.length === 1 ? t('1 agent') : t('{count} agents', { count: rows.length })}</Text>
          <View style={styles.grow} />
          <Chevron open />
        </Pressable>
        <View style={styles.stats}>
          {counts.map(({ group, count }) => (
            <View key={group} style={styles.stat}>
              <Text style={[styles.statCount, inkOf(group, count, colors, colors.ink)]}>{count}</Text>
              <View style={styles.statLabel}>
                <Dot color={tintOf(group, colors)} size={6} />
                <Text style={styles.caps} numberOfLines={1}>
                  {t(countWords(group))}
                </Text>
              </View>
            </View>
          ))}
        </View>
        <ScrollView style={{ maxHeight: tall * 0.5 }} contentContainerStyle={styles.body} showsVerticalScrollIndicator={false}>
          {listed('waitingOnYou')}
          {listed('readyToLook')}
          {moving.length ? (
            <View>
              <Label words={t(groupWords('moving'))} count={moving.length} />
              <Cluster rows={rows} moving={moving} agent={agent} onOpen={onOpen} />
            </View>
          ) : null}
          {listed('notSure')}
          {closed.length ? (
            <View>
              <Label words={t('Closed lately')} count={closed.length} />
              {closed.slice(0, CLOSED_SHOWN).map((entry) => (
                <ClosedLine key={entry.agent} closed={entry} connection={connection} full={full} clock={clock(entry.at)} />
              ))}
            </View>
          ) : null}
        </ScrollView>
      </View>
    </View>
  );
}

/** The rows of `group` the card counts and lists: a lead's workers stand under their lead, except those waiting on the person. */
function shownOf(rows: NowRow[], group: NowGroup): NowRow[] {
  return rows.filter((row) => row.group === group && (!row.lead || group === 'waitingOnYou'));
}

function workersOf(rows: NowRow[], lead: string): number {
  return rows.filter((row) => row.lead === lead && row.group !== 'waitingOnYou').length;
}

const GROUPS: NowGroup[] = ['waitingOnYou', 'readyToLook', 'moving', 'notSure'];

function countsOf(rows: NowRow[]) {
  return GROUPS.map((group) => ({ group, count: shownOf(rows, group).length }));
}

function tintOf(group: NowGroup, colors: Palette): string {
  switch (group) {
    case 'waitingOnYou':
      return colors.accent;
    case 'readyToLook':
      return colors.live;
    case 'moving':
      return colors.tertiary;
    default:
      return colors.warn;
  }
}

/** A count's ink: faint at zero, the accent for what waits on the person, else `rest`. */
function inkOf(group: NowGroup, count: number, colors: Palette, rest: string) {
  return { color: count === 0 ? colors.inkFaint : group === 'waitingOnYou' ? colors.accent : rest };
}

function countWords(group: NowGroup): string {
  switch (group) {
    case 'waitingOnYou':
      return 'For you';
    case 'readyToLook':
      return 'Ready';
    case 'moving':
      return 'Moving';
    default:
      return 'Not sure';
  }
}

function groupWords(group: NowGroup): string {
  switch (group) {
    case 'waitingOnYou':
      return 'Waiting on you';
    case 'readyToLook':
      return 'Ready to look at';
    case 'moving':
      return 'Moving';
    default:
      return 'Not sure';
  }
}

function nameOf(agent: WorkspaceAgent): string {
  return agent.name ? `@${agent.name}` : agent.title;
}

/** The one thing a row says of its agent, in the Mac's words. */
function factWords(fact: NowFact, t: Say): string {
  switch (fact.kind) {
    case 'asks':
      return fact.count === 1 ? t('asks you something') : t('asked you {count} things', { count: fact.count });
    case 'failed':
      return t('its turn failed');
    case 'stopped':
      return t('stopped, nothing set to wake it');
    case 'working':
      return fact.on ?? t('working');
    case 'waiting':
      if (fact.place) return inLine(fact.place, t);
      if (fact.on === 'ci') return t('waiting on CI');
      if (fact.on === 'agent' && fact.what) return t('waiting on @{who}', { who: fact.what });
      if (fact.what) return t('waiting on {what}', { what: fact.what });
      return t('waiting');
    case 'work':
      switch (fact.at) {
        case 'running':
          return t('in your app');
        case 'released':
          return fact.release ? t('in {tag}', { tag: fact.release }) : t('released');
        case 'landed':
          return t('landed');
        case 'committed':
          return t('committed, not landed');
        case 'uncommitted':
          return t('changes not committed');
        case 'staged':
          return t('on staging');
        case 'shipped':
          return t('shipped');
        default:
          return t('done');
      }
    case 'answered':
      return t('answered');
    default:
      return '';
  }
}

function closedWords(closed: NowClosed, t: Say): string {
  if (closed.why === 'landed') return t('landed');
  if (closed.why === 'nothingChanged') return t('changed nothing');
  if (closed.why === 'byPerson') return t('closed by you');
  if (closed.work === 'uncommitted') return t('left changes uncommitted');
  if (closed.work === 'committed') return t('committed, not landed');
  return t('closed');
}

function Chevron({ open }: { open: boolean }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.chevron}>
      <View style={{ transform: [{ rotate: open ? '-90deg' : '90deg' }] }}>
        <Icon name="IconChevron" size={11} color={colors.tertiary} />
      </View>
    </View>
  );
}

function Label({ words, count }: { words: string; count: number }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.label}>
      <Text style={styles.caps}>{words}</Text>
      <Text style={styles.labelCount}>{count}</Text>
    </View>
  );
}

function AgentPill({ agent }: { agent: WorkspaceAgent }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.agentPill}>
      <AgentFace agentId={agent.id} provider={agent.provider} size={14} />
      <Text style={styles.agentName} numberOfLines={1}>
        {nameOf(agent)}
      </Text>
    </View>
  );
}

function Marks({ agents }: { agents: WorkspaceAgent[] }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.marks}>
      {agents.slice(0, 4).map((agent, at) => (
        <View key={agent.id} style={[styles.mini, at > 0 && { marginLeft: -4 }]}>
          <AgentFace agentId={agent.id} provider={agent.provider} size={12} />
        </View>
      ))}
    </View>
  );
}

/** A moving agent's live dot, breathing while the app may animate. */
function LiveDot() {
  const styles = useStyles(makeStyles);
  const animating = useAnimating();
  const [phase] = useState(() => new Animated.Value(0));
  useEffect(() => {
    if (!animating) {
      phase.setValue(0);
      return;
    }
    const loop = Animated.loop(
      Animated.timing(phase, { toValue: 1, duration: 2400, easing: Easing.inOut(Easing.ease), useNativeDriver: true }),
    );
    loop.start();
    return () => loop.stop();
  }, [phase, animating]);
  const opacity = phase.interpolate({ inputRange: [0, 0.5, 1], outputRange: [1, 0.45, 1] });
  return <Animated.View style={[styles.liveDot, { opacity }]} />;
}

/** The moving agents as marks with live dots, a lead as a chip naming how many work under it, then what the longest standing does. */
function Cluster({
  rows,
  moving,
  agent,
  onOpen,
}: {
  rows: NowRow[];
  moving: NowRow[];
  agent: (id: string) => WorkspaceAgent | undefined;
  onOpen: (agentId: string) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const first = moving.map((row) => ({ row, known: agent(row.agent) })).find((found) => found.known);
  return (
    <View style={styles.cluster}>
      <View style={styles.stars}>
        {moving.map((row) => {
          const known = agent(row.agent);
          if (!known) return null;
          const under = workersOf(rows, known.id);
          return (
            <Pressable
              key={row.agent}
              onPress={() => onOpen(row.agent)}
              style={({ pressed }) => [styles.star, under > 0 && styles.lead, pressed && { backgroundColor: colors.selected }]}
              accessibilityRole="button"
              accessibilityLabel={`${nameOf(known)} · ${factWords(row.fact, t)}`}>
              <AgentFace agentId={known.id} provider={known.provider} size={13} />
              {under > 0 ? (
                <>
                  <Text style={styles.leadName}>{nameOf(known)}</Text>
                  <Text style={styles.leadMore}>{`+${under}`}</Text>
                </>
              ) : null}
              <LiveDot />
            </Pressable>
          );
        })}
      </View>
      {first?.known ? (
        <Text style={styles.who} numberOfLines={1}>
          {`${nameOf(first.known)} ${factWords(first.row.fact, t)}`}
        </Text>
      ) : null}
    </View>
  );
}

function ClosedLine({
  closed,
  connection,
  full,
  clock,
}: {
  closed: NowClosed;
  connection: ConnectionLike | undefined;
  full: boolean;
  clock: ReactNode;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const [sending, setSending] = useState(false);
  const [refused, setRefused] = useState<string>();
  const reopen = () => {
    if (sending) return;
    if (!connection) return setRefused(t(NOT_CONNECTED));
    haptics.tap();
    setSending(true);
    setRefused(undefined);
    callHost(connection, 'reopenAgent', { agentId: closed.agent })
      .catch((error: unknown) => setRefused(problem(error)))
      .finally(() => setSending(false));
  };
  return (
    <View>
      <View style={styles.row}>
        <Text style={styles.fact} numberOfLines={1}>
          {t('Closed @{name}, {why}', { name: closed.name, why: closedWords(closed, t) })}
        </Text>
        {full ? (
          <Pressable onPress={reopen} disabled={sending} hitSlop={8} style={styles.reopen} accessibilityRole="button">
            <Text style={[styles.reopenText, sending && { opacity: 0.5 }]}>{t('Reopen')}</Text>
          </Pressable>
        ) : null}
        {clock}
      </View>
      {refused ? <Text style={[styles.refused, { color: colors.danger }]}>{refused}</Text> : null}
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    openPlace: { position: 'absolute', top: 8, left: 12, right: 12, zIndex: 3 },
    foldedPlace: { position: 'absolute', top: 8, right: 14, zIndex: 3 },
    face: {
      backgroundColor: colors.raised,
      borderWidth: StyleSheet.hairlineWidth,
      borderColor: colors.borderStrong,
      shadowColor: '#000',
      shadowOpacity: 0.55,
      shadowRadius: 24,
      shadowOffset: { width: 0, height: 12 },
      elevation: 12,
    },
    card: { borderRadius: 14, overflow: 'hidden' },
    capsule: { flexDirection: 'row', alignItems: 'center', gap: 4, height: 34, paddingLeft: 5, paddingRight: 4, borderRadius: 17 },
    pill: { flexDirection: 'row', alignItems: 'center', gap: 6, height: 24, paddingHorizontal: 9, borderRadius: 12 },
    pillYou: { backgroundColor: colors.selected },
    pillText: { fontFamily: fonts.uiSemibold, fontSize: 13, fontVariant: ['tabular-nums'] },
    chevron: { width: 24, height: 24, alignItems: 'center', justifyContent: 'center' },
    head: { flexDirection: 'row', alignItems: 'center', gap: 8, height: 44, paddingLeft: 14, paddingRight: 10 },
    title: { fontFamily: fonts.uiSemibold, fontSize: 14, color: colors.ink },
    sub: { fontFamily: fonts.ui, fontSize: 13, color: colors.tertiary },
    grow: { flex: 1 },
    stats: {
      flexDirection: 'row',
      paddingHorizontal: 8,
      paddingTop: 2,
      paddingBottom: 8,
      borderBottomWidth: 1,
      borderBottomColor: colors.border,
    },
    stat: { flex: 1, paddingHorizontal: 8, paddingVertical: 4 },
    statCount: { fontFamily: fonts.uiSemibold, fontSize: 24, lineHeight: 28, letterSpacing: -0.5, fontVariant: ['tabular-nums'] },
    statLabel: { flexDirection: 'row', alignItems: 'center', gap: 6, marginTop: 3 },
    caps: { fontFamily: fonts.uiMedium, fontSize: 10, letterSpacing: 1.6, textTransform: 'uppercase', color: colors.tertiary },
    body: { paddingHorizontal: 6, paddingTop: 2, paddingBottom: 8 },
    label: { flexDirection: 'row', alignItems: 'center', gap: 8, height: 26, marginTop: 4, paddingHorizontal: 10 },
    labelCount: { fontFamily: fonts.uiMedium, fontSize: 12, color: colors.secondary, fontVariant: ['tabular-nums'] },
    row: { flexDirection: 'row', alignItems: 'center', gap: 9, minHeight: 34, paddingHorizontal: 10, borderRadius: 8 },
    fact: { flex: 1, minWidth: 0, fontFamily: fonts.ui, fontSize: 13.5, color: colors.secondary },
    clock: { fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary, fontVariant: ['tabular-nums'] },
    agentPill: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 5,
      height: 23,
      paddingLeft: 5,
      paddingRight: 8,
      borderRadius: 7,
      backgroundColor: colors.active,
      maxWidth: 150,
    },
    agentName: { fontFamily: fonts.uiMedium, fontSize: 13, color: colors.ink },
    marks: { flexDirection: 'row', alignItems: 'center' },
    mini: {
      width: 20,
      height: 20,
      borderRadius: 6,
      borderWidth: 2,
      borderColor: colors.raised,
      backgroundColor: colors.overlay,
      alignItems: 'center',
      justifyContent: 'center',
    },
    cluster: { gap: 6, paddingHorizontal: 10, paddingTop: 2, paddingBottom: 6 },
    stars: { flexDirection: 'row', flexWrap: 'wrap', gap: 6 },
    star: {
      minWidth: 28,
      height: 28,
      borderRadius: 8,
      backgroundColor: colors.active,
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'center',
    },
    lead: { paddingLeft: 7, paddingRight: 8, gap: 5 },
    leadName: { fontFamily: fonts.uiMedium, fontSize: 12.5, color: colors.secondary },
    leadMore: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary, fontVariant: ['tabular-nums'] },
    liveDot: {
      position: 'absolute',
      right: -2,
      bottom: -2,
      width: 8,
      height: 8,
      borderRadius: 4,
      borderWidth: 2,
      borderColor: colors.raised,
      backgroundColor: colors.live,
    },
    who: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
    reopen: { height: 26, paddingHorizontal: 8, borderRadius: 7, justifyContent: 'center' },
    reopenText: { fontFamily: fonts.uiMedium, fontSize: 13, color: colors.ink },
    refused: { paddingHorizontal: 10, paddingBottom: 4, fontFamily: fonts.ui, fontSize: 12.5 },
  });
