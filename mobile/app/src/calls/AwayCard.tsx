import { useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { answerBelow, AWAY_NEEDS_YOU, awayFor, DECIDED_FOR_YOU, LANDED, NOT_CONNECTED, OVERRULE, WHILE_AWAY } from '@shared/calls/words';
import { formatAge } from '@shared/lib/format';
import type { AwayDecision, ChiefAway } from '@shared/wire.generated';
import type { ConnectionLike } from '@sikemux/native';
import { AskerMark } from '@/calls/AskerMark';
import { callHost } from '@/core/hostCalls';
import { problem } from '@/core/problem';
import { useT } from '@/i18n';
import { Icon } from '@/ui/Icon';
import { useMinute } from '@/ui/motion';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/**
 * What happened while the person was away, at the end of the chief's chat, as the Mac draws it: the calls waiting,
 * which are the queue docked above the composer, what agents decided for them, with Overrule on a phone with full
 * access, and what landed, each row opening that agent's chat.
 */
export function AwayCard({
  away,
  connection,
  full,
  providerOf,
  onOpen,
}: {
  away: ChiefAway;
  connection: ConnectionLike | undefined;
  full: boolean;
  providerOf: (agentId: string) => string;
  onOpen: (agentId: string) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const minute = useMinute();
  const calls = away.calls ?? [];
  const decided = away.decided ?? [];
  const landed = away.landed ?? [];
  return (
    <View style={styles.card}>
      <View style={styles.head}>
        <Icon name="Logo" size={15} color={colors.accent} />
        <Text style={styles.title}>{t(WHILE_AWAY)}</Text>
        {away.back ? <Text style={styles.gone}>{`· ${awayFor(away.back - away.since, t)}`}</Text> : null}
      </View>
      {calls.length ? (
        <>
          <Section label={t(AWAY_NEEDS_YOU)} count={calls.length} />
          <View style={styles.calls}>
            <View style={[styles.pip, { backgroundColor: colors.warn }]} />
            <Text style={styles.callsText}>{answerBelow(calls.length, t)}</Text>
          </View>
        </>
      ) : null}
      {decided.length ? (
        <>
          <Section label={t(DECIDED_FOR_YOU)} count={decided.length} />
          {decided.map((decision, index) => (
            <Decision
              key={`${decision.goal ?? ''}/${decision.decision}`}
              decision={decision}
              last={index === decided.length - 1}
              connection={connection}
              full={full}
            />
          ))}
        </>
      ) : null}
      {landed.length ? (
        <>
          <Section label={t(LANDED)} count={landed.length} />
          {landed.map((work, index) => (
            <Pressable
              key={`${work.agentId}/${work.at}`}
              onPress={() => onOpen(work.agentId)}
              style={[styles.row, index === landed.length - 1 && styles.lastRow]}
              accessibilityRole="button">
              <View style={styles.mark}>
                <AskerMark agentId={work.agentId} provider={providerOf(work.agentId)} size={16} />
              </View>
              <View style={styles.text}>
                <Text style={styles.what} numberOfLines={2}>
                  {work.title}
                </Text>
                <Text style={styles.under} numberOfLines={1}>
                  {[work.name ? `@${work.name}` : null, formatAge(work.at, minute * 60_000, t)].filter(Boolean).join(' · ')}
                </Text>
              </View>
              <Icon name="IconChevron" size={12} color={colors.tertiary} />
            </Pressable>
          ))}
        </>
      ) : null}
    </View>
  );
}

function Section({ label, count }: { label: string; count: number }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.section}>
      <Text style={styles.sectionText}>{`${label} ${count}`}</Text>
      <View style={styles.sectionRule} />
    </View>
  );
}

/** A decision made for the person, struck once they overrule it; Overrule only where the phone may act. */
function Decision({
  decision,
  last,
  connection,
  full,
}: {
  decision: AwayDecision;
  last: boolean;
  connection: ConnectionLike | undefined;
  full: boolean;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const [sending, setSending] = useState(false);
  const [refused, setRefused] = useState<string>();
  const under = decision.why ?? (decision.by ? `@${decision.by.name}` : undefined);
  const overrule = () => {
    if (sending) return;
    if (!connection) return setRefused(t(NOT_CONNECTED));
    setSending(true);
    setRefused(undefined);
    callHost(connection, 'overruleDecision', { project: decision.project, goal: decision.goal ?? null, decision: decision.decision })
      .catch((error: unknown) => setRefused(problem(error)))
      .finally(() => setSending(false));
  };
  return (
    <View style={[styles.row, last && styles.lastRow]}>
      <View style={styles.mark}>
        <Icon name="IconCheck" size={14} color={colors.tertiary} />
      </View>
      <View style={styles.text}>
        <Text style={[styles.what, decision.overruled && styles.struck]} numberOfLines={2}>
          {decision.text}
        </Text>
        {under ? (
          <Text style={styles.under} numberOfLines={1}>
            {under}
          </Text>
        ) : null}
        {refused ? <Text style={[styles.under, { color: colors.danger }]}>{refused}</Text> : null}
      </View>
      {full && !decision.overruled ? (
        <Pressable onPress={overrule} disabled={sending} style={[styles.act, sending && { opacity: 0.5 }]} accessibilityRole="button">
          <Text style={styles.actText}>{t(OVERRULE)}</Text>
        </Pressable>
      ) : null}
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    card: {
      marginTop: 12,
      marginBottom: 8,
      paddingHorizontal: 12,
      paddingTop: 12,
      paddingBottom: 6,
      borderRadius: 16,
      borderWidth: 1,
      borderColor: colors.borderStrong,
      backgroundColor: colors.overlay,
    },
    head: { flexDirection: 'row', alignItems: 'center', gap: 8 },
    title: { fontFamily: fonts.uiSemibold, fontSize: 14.5, color: colors.ink },
    gone: { fontFamily: fonts.ui, fontSize: 13, color: colors.tertiary },
    section: { flexDirection: 'row', alignItems: 'center', gap: 10, marginTop: 14, marginBottom: 2 },
    sectionText: { fontFamily: fonts.uiMedium, fontSize: 11, letterSpacing: 2.2, textTransform: 'uppercase', color: colors.tertiary },
    sectionRule: { flex: 1, height: 1, backgroundColor: colors.border },
    calls: { flexDirection: 'row', alignItems: 'center', gap: 8, minHeight: 40 },
    pip: { width: 8, height: 8, borderRadius: 4 },
    callsText: { fontFamily: fonts.ui, fontSize: 14, color: colors.secondary },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      paddingVertical: 9,
      minHeight: 44,
      borderBottomWidth: 1,
      borderBottomColor: colors.border,
    },
    lastRow: { borderBottomWidth: 0 },
    mark: { width: 16, alignSelf: 'flex-start', marginTop: 3, alignItems: 'center' },
    text: { flex: 1, minWidth: 0 },
    what: { fontFamily: fonts.ui, fontSize: 14.5, lineHeight: 20, color: colors.ink },
    struck: { textDecorationLine: 'line-through', color: colors.tertiary },
    under: { marginTop: 2, fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
    act: {
      height: 32,
      paddingHorizontal: 12,
      borderRadius: 9,
      borderWidth: 1,
      borderColor: colors.borderStrong,
      justifyContent: 'center',
    },
    actText: { fontFamily: fonts.uiMedium, fontSize: 13.5, color: colors.ink },
  });
