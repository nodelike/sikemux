import { useContext, useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { getCalendars } from 'expo-localization';

import {
  DECIDED,
  FROM_NOW_ON,
  NOT_CONNECTED,
  OVERRULE,
  OVERRULED,
  OVERRULED_UNDONE,
  quoted,
  ruleChange,
  STANDS_AGAIN,
  STOPPED_AUTO,
  UNDO,
  weekCame,
  weekCount,
} from '@shared/calls/words';
import type { ChiefWeek, DecidedLine, OverruledDecision, RuleChange } from '@shared/wire.generated';
import { callHost } from '@/core/hostCalls';
import { problem } from '@/core/problem';
import { connectionOf, liveOf } from '@/devices/hub';
import { HostContext } from '@/chat/hostFiles';
import { ActsContext } from '@/chat/transcriptContext';
import { useT } from '@/i18n';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/**
 * A decision's quiet line in a chat: what became of it with its one action, the decision quoted, then why. The decided
 * line and the Overruled line it turns into share it.
 */
function DecisionLine({
  icon,
  head,
  action,
  act,
  sending,
  quote,
  why,
  refused,
}: {
  icon: 'IconCheck' | 'IconReply';
  head: string;
  action?: string;
  act: () => void;
  sending: boolean;
  quote?: string;
  why?: string;
  refused?: string;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  return (
    <View style={styles.line}>
      <View style={styles.head}>
        <Icon name={icon} size={12} color={colors.tertiary} />
        <Text style={styles.quiet}>{head}</Text>
        {action ? (
          <Pressable onPress={act} disabled={sending} hitSlop={10} style={styles.undo} accessibilityRole="button">
            <Text style={[styles.undoText, sending && { opacity: 0.5 }]}>{action}</Text>
          </Pressable>
        ) : null}
      </View>
      {quote ? (
        <Text style={styles.quoted} numberOfLines={2}>
          {quoted(quote, t)}
        </Text>
      ) : null}
      {why ? (
        <Text style={styles.why} numberOfLines={2}>
          {why}
        </Text>
      ) : null}
      {refused ? <Text style={[styles.why, { color: colors.danger }]}>{refused}</Text> : null}
    </View>
  );
}

/** Sends `method` for a decision to the chat's host, and what the host refused, if it did. */
function useDecisionCall(method: 'overruleDecision' | 'undoOverrule', decision: OverruledDecision | DecidedLine) {
  const t = useT();
  const core = useContext(HostContext);
  const [sending, setSending] = useState(false);
  const [refused, setRefused] = useState<string>();
  const send = () => {
    if (sending) return;
    const connection = connectionOf(liveOf(core));
    if (!connection) return setRefused(t(NOT_CONNECTED));
    setSending(true);
    setRefused(undefined);
    callHost(connection, method, { project: decision.project, goal: decision.goal ?? null, decision: decision.decision })
      .catch((error: unknown) => setRefused(problem(error)))
      .finally(() => setSending(false));
  };
  return { send, sending, refused };
}

/**
 * The line under an agent's reply that says it recorded a decision itself, as the Mac words it: Decided, with Overrule
 * on a phone with full access, the decision quoted and what it rests on.
 */
export function DecidedLineRow({ decided }: { decided: DecidedLine }) {
  const t = useT();
  const acts = useContext(ActsContext);
  const { send, sending, refused } = useDecisionCall('overruleDecision', decided);
  return (
    <DecisionLine
      icon="IconCheck"
      head={t(DECIDED)}
      action={acts ? t(OVERRULE) : undefined}
      act={send}
      sending={sending}
      quote={decided.text}
      why={decided.why}
      refused={refused}
    />
  );
}

/**
 * The quiet line an Overrule leaves in the chief's chat, as the Mac words it: what was decided, quoted, and Undo until
 * it is undone, or when it stopped Auto's run, which Undo cannot put back.
 */
export function OverruledLine({ overruled }: { overruled: OverruledDecision }) {
  const t = useT();
  const acts = useContext(ActsContext);
  const { send, sending, refused } = useDecisionCall('undoOverrule', overruled);
  const why = overruled.undone
    ? t(STANDS_AGAIN)
    : overruled.stoppedAuto
      ? t(STOPPED_AUTO)
      : overruled.rule !== undefined
        ? t(FROM_NOW_ON)
        : undefined;
  const offersUndo = acts && !overruled.undone && !overruled.stoppedAuto;
  return (
    <DecisionLine
      icon="IconReply"
      head={t(overruled.undone ? OVERRULED_UNDONE : OVERRULED)}
      action={offersUndo ? t(UNDO) : undefined}
      act={send}
      sending={sending}
      quote={overruled.text}
      why={why}
      refused={refused}
    />
  );
}

/** The chief's count of last week, in its own words under the day and time it came. */
export function WeekLine({ week }: { week: ChiefWeek }) {
  const styles = useStyles(makeStyles);
  const t = useT();
  return (
    <View style={styles.week}>
      <Text style={styles.when}>{weekCame(week.to, t, getCalendars()[0]?.uses24hourClock === false)}</Text>
      <Text style={styles.count}>{weekCount(week, t)}</Text>
    </View>
  );
}

/**
 * The quiet line a rule change leaves, as the Mac words it on one line: its title, the rule quoted, which gives way
 * first, then the check's short hash or who did it. The phone offers no Undo on it.
 */
export function RuleLine({ line }: { line: RuleChange }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const { head, tail } = ruleChange(line, t);
  const rule = quoted(line.text, t);
  return (
    <View style={[styles.head, styles.rule]} accessible accessibilityLabel={[head, rule, tail].filter(Boolean).join(', ')}>
      <Icon name="IconCheck" size={12} color={colors.tertiary} />
      <Text style={styles.quiet}>{head}</Text>
      <Text style={[styles.quiet, styles.ruleText]} numberOfLines={1}>
        {`· ${rule}`}
      </Text>
      {tail ? <Text style={styles.quiet}>{`· ${tail}`}</Text> : null}
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    line: { gap: 4, marginVertical: 10, paddingVertical: 2, paddingLeft: 12 },
    head: { flexDirection: 'row', alignItems: 'center', gap: 8 },
    quiet: { fontFamily: fonts.ui, fontSize: 13, color: colors.tertiary },
    undo: { marginLeft: 'auto', paddingLeft: 12 },
    undoText: { fontFamily: fonts.uiMedium, fontSize: 13, color: colors.ink },
    quoted: { fontFamily: fonts.ui, fontSize: 14, lineHeight: 20, color: colors.ink },
    why: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
    rule: { marginVertical: 6, paddingLeft: 12 },
    ruleText: { flexShrink: 1, color: colors.ink },
    week: { marginVertical: 12 },
    when: { fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
    count: { marginTop: 4, fontFamily: fonts.ui, fontSize: 14.5, lineHeight: 22.8, color: colors.ink },
  });
