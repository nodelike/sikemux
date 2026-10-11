import type { ReactNode } from 'react';
import { Pressable, StyleSheet, Text, View, type StyleProp, type ViewStyle } from 'react-native';

import { Icon } from './Icon';
import { fonts, type Palette, translucent, typeFor, useColors, useStyles } from './theme';

/** A list's small-caps label, with a hairline running to the right as on the Mac's rail. */
export function SectionLabel({ children, count, action }: { children: string; count?: number; action?: ReactNode }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.sectionLabel}>
      <Text style={styles.sectionLabelText}>{children}</Text>
      {count !== undefined ? <Text style={styles.sectionCount}>{count}</Text> : null}
      <View style={styles.sectionRule} />
      {action}
    </View>
  );
}

/** Rows straight on the ground, a little apart, the way the Mac's rail lists agents. */
export function Rows({ children, style }: { children: ReactNode; style?: StyleProp<ViewStyle> }) {
  const styles = useStyles(makeStyles);
  return <View style={[styles.rows, style]}>{children}</View>;
}

export function Row({
  mark,
  title,
  detail,
  place,
  end,
  onPress,
  dim,
}: {
  mark?: ReactNode;
  title: string;
  detail?: ReactNode;
  /** The project the row belongs to, marked with the folder the Mac's rail puts beside it. */
  place?: string;
  end?: ReactNode;
  onPress?: () => void;
  dim?: boolean;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <Pressable
      onPress={onPress}
      accessibilityRole={onPress ? 'button' : undefined}
      style={({ pressed }) => [styles.row, pressed && onPress && styles.rowPressed]}>
      {mark ? <View style={styles.mark}>{mark}</View> : null}
      <View style={styles.rowBody}>
        <Text style={[styles.rowTitle, { color: dim ? colors.tertiary : colors.ink }]} numberOfLines={1}>
          {title}
        </Text>
        {place ? (
          <View style={styles.rowPlace}>
            <Icon name="IconFolder" size={11} color={colors.live} />
            <Text style={[styles.rowDetail, styles.rowPlaceText]} numberOfLines={1}>
              {place}
              {detail ? <> · {detail}</> : null}
            </Text>
          </View>
        ) : detail ? (
          <Text style={styles.rowDetail} numberOfLines={1}>
            {detail}
          </Text>
        ) : null}
      </View>
      {end ? <View style={styles.rowEnd}>{end}</View> : null}
    </Pressable>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    sectionLabel: { flexDirection: 'row', alignItems: 'center', gap: 10, paddingTop: 18, paddingBottom: 6, paddingHorizontal: 8 },
    sectionLabelText: { fontFamily: fonts.uiMedium, fontSize: 11, letterSpacing: 1.76, textTransform: 'uppercase', color: colors.tertiary },
    sectionCount: { marginLeft: -4, fontFamily: fonts.uiMedium, fontSize: 11, color: colors.inkDim, fontVariant: ['tabular-nums'] },
    sectionRule: { flex: 1, height: 1, backgroundColor: translucent(colors.ink, 0.09) },
    rows: { gap: 2 },
    row: { minHeight: 48, flexDirection: 'row', alignItems: 'center', gap: 10, paddingHorizontal: 8, paddingVertical: 6, borderRadius: 9 },
    rowPressed: { backgroundColor: colors.active },
    mark: { width: 20, height: 20, alignItems: 'center', justifyContent: 'center' },
    rowBody: { flex: 1, minWidth: 0 },
    rowTitle: { ...type.row, fontSize: 14.5 },
    rowDetail: { ...type.meta, fontSize: 12, marginTop: 1 },
    rowPlace: { flexDirection: 'row', alignItems: 'center', gap: 4, marginTop: 1 },
    rowPlaceText: { flexShrink: 1, marginTop: 0 },
    rowEnd: { minWidth: 20, alignItems: 'center', justifyContent: 'center', flexDirection: 'row', gap: 6 },
  });
};
