import { useContext, useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import {
  FROM_NOW_ON,
  NOT_CONNECTED,
  OVERRULED,
  OVERRULED_UNDONE,
  quoted,
  STANDS_AGAIN,
  STOPPED_AUTO,
  UNDO,
  weekCame,
  weekCount,
} from '@shared/calls/words';
import type { ChiefWeek, OverruledDecision } from '@shared/wire.generated';
import { callHost } from '@/core/hostCalls';
import { problem } from '@/core/problem';
import { connectionOf, liveOf } from '@/devices/hub';
import { HostContext } from '@/chat/hostFiles';
import { ActsContext } from '@/chat/transcriptContext';
import { useT } from '@/i18n';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/**
 * The quiet line an Overrule leaves in the chief's chat, as the Mac words it: what was decided, quoted, and Undo until
 * it is undone, or when it stopped Auto's run, which Undo cannot put back.
 */
export function OverruledLine({ overruled }: { overruled: OverruledDecision }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const core = useContext(HostContext);
  const acts = useContext(ActsContext);
  const [sending, setSending] = useState(false);
  const [refused, setRefused] = useState<string>();
  const why = overruled.undone
    ? t(STANDS_AGAIN)
    : overruled.stoppedAuto
      ? t(STOPPED_AUTO)
      : overruled.rule !== undefined
        ? t(FROM_NOW_ON)
        : undefined;
  const offersUndo = acts && !overruled.undone && !overruled.stoppedAuto;
  const undo = () => {
    if (sending) return;
    const connection = connectionOf(liveOf(core));
    if (!connection) return setRefused(t(NOT_CONNECTED));
    setSending(true);
    setRefused(undefined);
    callHost(connection, 'undoOverrule', { project: overruled.project, goal: overruled.goal ?? null, decision: overruled.decision })
      .catch((error: unknown) => setRefused(problem(error)))
      .finally(() => setSending(false));
  };
  return (
    <View style={styles.line}>
      <View style={styles.head}>
        <Icon name="IconReply" size={12} color={colors.tertiary} />
        <Text style={styles.quiet}>{t(overruled.undone ? OVERRULED_UNDONE : OVERRULED)}</Text>
        {offersUndo ? (
          <Pressable onPress={undo} disabled={sending} hitSlop={10} style={styles.undo} accessibilityRole="button">
            <Text style={[styles.undoText, sending && { opacity: 0.5 }]}>{t(UNDO)}</Text>
          </Pressable>
        ) : null}
      </View>
      {overruled.text ? (
        <Text style={styles.quoted} numberOfLines={2}>
          {quoted(overruled.text, t)}
        </Text>
      ) : null}
      {why ? <Text style={styles.why}>{why}</Text> : null}
      {refused ? <Text style={[styles.why, { color: colors.danger }]}>{refused}</Text> : null}
    </View>
  );
}

/** The chief's count of last week, in its own words under the day and time it came. */
export function WeekLine({ week }: { week: ChiefWeek }) {
  const styles = useStyles(makeStyles);
  const t = useT();
  return (
    <View style={styles.week}>
      <Text style={styles.when}>{weekCame(week.to, t)}</Text>
      <Text style={styles.count}>{weekCount(week, t)}</Text>
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
    week: { marginVertical: 12 },
    when: { fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
    count: { marginTop: 4, fontFamily: fonts.ui, fontSize: 14.5, lineHeight: 22.8, color: colors.ink },
  });
