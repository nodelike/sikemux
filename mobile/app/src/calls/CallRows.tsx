import { useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { formatAge } from '@shared/lib/format';
import { headline, newest as newestOf } from '@shared/calls/words';
import type { PersonCall } from '@shared/wire.generated';
import { AskerMark } from '@/calls/AskerMark';
import { useT } from '@/i18n';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { useMinute } from '@/ui/motion';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

const ROW = 44;

/**
 * Calls folded to a line each, as the Mac's chat folds them: the asker's mark, the question and how long it has waited.
 * Two or more fold under one line that names the newest, and a tap on it opens their lines.
 */
export function CallRows({
  calls,
  counted,
  opens = false,
  onPress,
}: {
  calls: PersonCall[];
  /** What the fold's line says of how many calls it holds. */
  counted: (count: number) => string;
  /** Each line ends on a chevron, as it opens a screen of its own. */
  opens?: boolean;
  onPress: (call: PersonCall) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const minute = useMinute();
  const t = useT();
  const [open, setOpen] = useState(false);
  const newest = calls.reduce<PersonCall | undefined>((last, call) => (!last || call.raisedAt > last.raisedAt ? call : last), undefined);
  const age = (call: PersonCall) => <Text style={styles.age}>{formatAge(call.raisedAt, minute * 60_000, t)}</Text>;
  const mark = (call: PersonCall) => (
    <AskerMark agentId={call.asker.agentId} provider={call.asker.cli} chief={call.asker.chief} asking size={16} />
  );
  return (
    <View>
      {calls.length > 1 && newest ? (
        <Pressable
          onPress={() => {
            haptics.select();
            setOpen(!open);
          }}
          style={styles.row}
          accessibilityRole="button"
          accessibilityState={{ expanded: open }}>
          {mark(newest)}
          <Text style={styles.text} numberOfLines={1}>
            <Text style={{ color: colors.ink }}>{counted(calls.length)}</Text>
            <Text style={{ color: colors.inkFaint }}> · </Text>
            {newestOf(headline(newest), t)}
          </Text>
          {age(newest)}
          <View style={{ transform: [{ rotate: open ? '90deg' : '0deg' }] }}>
            <Icon name="IconChevron" size={11} color={colors.tertiary} />
          </View>
        </Pressable>
      ) : null}
      {calls.length === 1 || open
        ? calls.map((call) => (
            <Pressable
              key={call.id}
              onPress={() => {
                haptics.select();
                onPress(call);
              }}
              style={styles.row}
              accessibilityRole="button">
              {mark(call)}
              <Text style={[styles.text, opens && { color: colors.ink }]} numberOfLines={1}>
                {headline(call)}
              </Text>
              {age(call)}
              {opens ? <Icon name="IconChevron" size={11} color={colors.tertiary} /> : null}
            </Pressable>
          ))
        : null}
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    row: { flexDirection: 'row', alignItems: 'center', gap: 10, minHeight: ROW },
    text: { flex: 1, fontFamily: fonts.ui, fontSize: 14, color: colors.secondary },
    age: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.warn },
  });
};
