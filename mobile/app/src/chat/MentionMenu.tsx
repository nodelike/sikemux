import { ActivityIndicator, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { haptics } from '@/ui/haptics';
import { AgentFace } from '@/devices/Buddy';
import { Icon } from '@/ui/Icon';
import { TurnMark } from '@/devices/AgentMarks';
import { useMinute } from '@/ui/motion';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import type { Trigger } from './mentions';
import type { MentionMark, MentionMenuView } from './useMentions';

const SCOPES: [Trigger, string][] = [
  ['@', 'Agents'],
  ['#', 'Issues'],
  ['!', 'Pull requests'],
  ['+', 'Files'],
];

function Mark({ mark }: { mark: MentionMark }) {
  const colors = useColors();
  if ('icon' in mark) return <Icon name={mark.icon} size={15} color={colors.tertiary} />;
  return <AgentFace agentId={mark.agent} provider={mark.provider} size={18} logoSize={16} />;
}

/**
 * The menu above the composer while its last word starts with `@`, `+`, `#` or `!`, under the kinds of thing
 * it names; a tap on a kind asks for that instead, and a tap on a row writes it into the draft.
 */
export function MentionMenu({ menu, onScope }: { menu: MentionMenuView; onScope: (trigger: Trigger) => void }) {
  const colors = useColors();
  const minute = useMinute();
  const styles = useStyles(makeStyles);
  const { typed, rows, empty } = menu;
  return (
    <View style={styles.menu} accessibilityRole="menu">
      <View style={styles.head}>
        <ScrollView
          horizontal
          showsHorizontalScrollIndicator={false}
          keyboardShouldPersistTaps="always"
          contentContainerStyle={styles.scopes}>
          {SCOPES.map(([key, label]) => {
            const on = key === typed.trigger;
            return (
              <Pressable
                key={key}
                onPress={() => {
                  if (on) return;
                  haptics.select();
                  onScope(key);
                }}
                style={[styles.scope, on && styles.scopeOn]}
                accessibilityRole="tab"
                accessibilityState={{ selected: on }}>
                <Text style={[styles.scopeKey, on && { color: colors.accent }]}>{key}</Text>
                <Text style={[styles.scopeText, on && { color: colors.ink }]}>{label}</Text>
              </Pressable>
            );
          })}
        </ScrollView>
      </View>
      {rows.length ? (
        <ScrollView style={styles.rows} keyboardShouldPersistTaps="always" nestedScrollEnabled>
          {rows.map((row) => (
            <View key={row.key}>
              {row.group ? <Text style={styles.group}>{row.group}</Text> : null}
              <Pressable
                onPress={() => {
                  haptics.select();
                  row.take();
                }}
                style={({ pressed }) => [styles.row, pressed && { backgroundColor: colors.active }]}
                accessibilityRole="menuitem"
                accessibilityLabel={[row.title, row.detail].filter(Boolean).join(', ')}>
                <View style={styles.mark}>
                  <Mark mark={row.mark} />
                </View>
                <Text style={[styles.title, row.plain && styles.plain]} numberOfLines={1}>
                  {row.title}
                </Text>
                {row.detail ? (
                  <Text style={styles.detail} numberOfLines={1}>
                    {row.detail}
                  </Text>
                ) : (
                  <View style={{ flex: 1 }} />
                )}
                {row.aside ? <Text style={styles.rowAside}>{row.aside}</Text> : null}
                {row.turn ? <TurnMark turn={row.turn} now={minute * 60_000} /> : null}
              </Pressable>
            </View>
          ))}
        </ScrollView>
      ) : (
        <View style={styles.none} accessibilityLiveRegion="polite">
          {empty ? <Text style={styles.detail}>{empty}</Text> : <ActivityIndicator size="small" color={colors.tertiary} />}
        </View>
      )}
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    menu: {
      marginBottom: 8,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.overlay,
      overflow: 'hidden',
    },
    head: { borderBottomWidth: 1, borderBottomColor: colors.border },
    scopes: { gap: 2, padding: 5 },
    scope: { flexDirection: 'row', alignItems: 'center', gap: 5, height: 30, paddingHorizontal: 9, borderRadius: 8 },
    scopeOn: { backgroundColor: colors.active },
    scopeKey: { fontFamily: fonts.mono, fontSize: 12.5, color: colors.inkFaint },
    scopeText: { fontFamily: fonts.uiMedium, fontSize: 12.5, color: colors.tertiary },
    rows: { maxHeight: 232, paddingVertical: 3 },
    group: {
      paddingTop: 9,
      paddingBottom: 3,
      paddingHorizontal: 14,
      fontFamily: fonts.uiSemibold,
      fontSize: 10.5,
      letterSpacing: 0.9,
      textTransform: 'uppercase',
      color: colors.inkFaint,
    },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      minHeight: 44,
      paddingHorizontal: 12,
      marginHorizontal: 3,
      borderRadius: 9,
    },
    mark: { width: 18, alignItems: 'center' },
    title: { flexShrink: 0, maxWidth: '55%', fontFamily: fonts.mono, fontSize: 13, color: colors.accent },
    plain: { maxWidth: '70%', fontFamily: fonts.ui, color: colors.ink },
    detail: { flex: 1, fontFamily: fonts.ui, fontSize: 13, color: colors.tertiary },
    rowAside: { fontFamily: fonts.ui, fontSize: 12, color: colors.inkFaint },
    none: { minHeight: 44, justifyContent: 'center', paddingHorizontal: 14 },
  });
