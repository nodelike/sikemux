import { type ReactNode, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, useWindowDimensions, View } from 'react-native';

import { inLine, NOT_CONNECTED } from '@shared/calls/words';
import { formatAge } from '@shared/lib/format';
import type { Say } from '@shared/lib/say';
import type { NowClosed, NowFact, NowGroup, NowRow, ProjectNow, WorkspaceAgent } from '@shared/wire.generated';
import type { ConnectionLike } from '@sikemux/native';
import Svg, { Defs, LinearGradient, Rect, Stop } from 'react-native-svg';
import { AgentFace } from '@/devices/Buddy';
import { Dot } from '@/devices/AgentMarks';
import { callHost } from '@/core/hostCalls';
import { problem } from '@/core/problem';
import { useT } from '@/i18n';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { useMinute } from '@/ui/motion';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/** Rows of a group shown before the rest fold into one line of marks. */
const SHOWN = 4;
const CLOSED_SHOWN = 3;
const LATE_MS = 10 * 60_000;

/**
 * The Now card over a chief's chat, as the Mac draws it: its project's agents in four groups, waiting on the person,
 * ready to look at, moving and not sure, one row each, then the agents closed lately with Reopen. Folded, it is the
 * four counts alone at the top right.
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
          <TopLight />
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
    const under = workersOf(rows, known.id);
    return (
      <Pressable
        key={row.agent}
        onPress={() => onOpen(row.agent)}
        style={({ pressed }) => [styles.row, pressed && { backgroundColor: colors.active }]}
        accessibilityRole="button">
        <AgentFace agentId={known.id} provider={known.provider} size={15} />
        <View style={styles.name}>
          <Text style={styles.agentName} numberOfLines={1}>
            {nameOf(known)}
          </Text>
          {under > 0 ? <Text style={styles.under}>{`+${under}`}</Text> : null}
        </View>
        <Text style={styles.fact} numberOfLines={1}>
          {factWords(row.fact, t)}
        </Text>
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
        <Label words={t(groupWords(group))} count={shown.length} tint={tintOf(group, colors)} />
        {shown.slice(0, SHOWN).map(factRow)}
        {rest(shown.slice(SHOWN))}
      </View>
    );
  };
  return (
    <View style={styles.openPlace} pointerEvents="box-none">
      <View style={[styles.face, styles.card]}>
        <TopLight />
        <Pressable onPress={fold} style={styles.head} accessibilityRole="button" accessibilityLabel={t('Fold Now')}>
          <Icon name="Logo" size={14} color={colors.accent} />
          <Text style={styles.title}>{t('Now')}</Text>
          <Text style={styles.sub}>{`· ${rows.length === 1 ? t('1 agent') : t('{count} agents', { count: rows.length })}`}</Text>
          <View style={styles.grow} />
          <Chevron open />
        </Pressable>
        <ScrollView style={{ maxHeight: tall * 0.5 }} contentContainerStyle={styles.body} showsVerticalScrollIndicator={false}>
          {GROUPS.map(listed)}
          {closed.length ? (
            <View>
              <Label words={t('Closed lately')} count={closed.length} tint={colors.tertiary} />
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

/** A group's spaced capitals in its colour, how many stand in it, and a hairline to the edge. */
function Label({ words, count, tint }: { words: string; count: number; tint: string }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.label}>
      <Text style={[styles.caps, { color: tint }]} numberOfLines={1}>
        {words}
      </Text>
      <Text style={styles.labelCount}>{count}</Text>
      <View style={styles.hairline} />
    </View>
  );
}

/** The faint light at the top of the card, as the Mac lifts its face. */
function TopLight() {
  const colors = useColors();
  return (
    <Svg style={StyleSheet.absoluteFill} pointerEvents="none">
      <Defs>
        <LinearGradient id="nowLight" x1="0" y1="0" x2="0" y2="1">
          <Stop offset="0" stopColor={colors.ink} stopOpacity={0.035} />
          <Stop offset="0.3" stopColor={colors.ink} stopOpacity={0} />
        </LinearGradient>
      </Defs>
      <Rect width="100%" height="100%" fill="url(#nowLight)" />
    </Svg>
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
      boxShadow: '0 8px 16px -8px rgba(0, 0, 0, 0.44), 0 28px 64px -16px rgba(0, 0, 0, 0.62)',
    },
    card: { borderRadius: 14, overflow: 'hidden' },
    capsule: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 4,
      height: 34,
      paddingLeft: 5,
      paddingRight: 4,
      borderRadius: 17,
      overflow: 'hidden',
    },
    pill: { flexDirection: 'row', alignItems: 'center', gap: 6, height: 24, paddingHorizontal: 9, borderRadius: 12 },
    pillYou: { backgroundColor: colors.selected },
    pillText: { fontFamily: fonts.uiSemibold, fontSize: 13, fontVariant: ['tabular-nums'] },
    chevron: { width: 24, height: 24, alignItems: 'center', justifyContent: 'center' },
    head: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      height: 48,
      paddingLeft: 16,
      paddingRight: 10,
      borderBottomWidth: 1,
      borderBottomColor: colors.border,
    },
    title: { fontFamily: fonts.uiSemibold, fontSize: 14, color: colors.ink },
    sub: { fontFamily: fonts.ui, fontSize: 13, color: colors.tertiary },
    grow: { flex: 1 },
    caps: { fontFamily: fonts.uiMedium, fontSize: 10, letterSpacing: 1.6, textTransform: 'uppercase', color: colors.tertiary },
    body: { gap: 10, paddingHorizontal: 8, paddingTop: 6, paddingBottom: 10 },
    label: { flexDirection: 'row', alignItems: 'center', gap: 8, height: 26, paddingHorizontal: 10 },
    labelCount: { fontFamily: fonts.ui, fontSize: 11.5, color: colors.tertiary, fontVariant: ['tabular-nums'] },
    hairline: { flex: 1, height: 1, backgroundColor: colors.border },
    row: { flexDirection: 'row', alignItems: 'center', gap: 10, minHeight: 36, paddingHorizontal: 10, borderRadius: 8 },
    name: { width: 96, flexDirection: 'row', alignItems: 'baseline', gap: 4 },
    under: { fontFamily: fonts.ui, fontSize: 11.5, color: colors.tertiary, fontVariant: ['tabular-nums'] },
    fact: { flex: 1, minWidth: 0, fontFamily: fonts.ui, fontSize: 13.5, color: colors.secondary },
    clock: { minWidth: 30, textAlign: 'right', fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary, fontVariant: ['tabular-nums'] },
    agentName: { flexShrink: 1, fontFamily: fonts.uiMedium, fontSize: 13.5, color: colors.ink },
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
    reopen: { height: 26, paddingHorizontal: 8, borderRadius: 7, justifyContent: 'center' },
    reopenText: { fontFamily: fonts.uiMedium, fontSize: 13, color: colors.ink },
    refused: { paddingHorizontal: 10, paddingBottom: 4, fontFamily: fonts.ui, fontSize: 12.5 },
  });
