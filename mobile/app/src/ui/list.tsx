import { type ReactNode, useState } from 'react';
import { Pressable, StyleSheet, Text, useWindowDimensions, View, type StyleProp, type ViewStyle } from 'react-native';

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

export function Rows({ children, style }: { children: ReactNode; style?: StyleProp<ViewStyle> }) {
  const styles = useStyles(makeStyles);
  return <View style={[styles.rows, style]}>{children}</View>;
}

type Measure = { of: string; width: number };
type Fit = { title: Measure; model: Measure; body: Measure & { withModel: boolean } };
const unmeasured: Measure = { of: '', width: 0 };

export function Row({
  mark,
  name,
  model,
  host,
  title,
  aside,
  detail,
  place,
  end,
  onPress,
  onLongPress,
  dim,
  detailLines = 1,
  under,
  after,
}: {
  mark?: ReactNode;
  /** The agent's name, small and dim with the row's counts and state at its end, as the Mac's rail sets it. */
  name?: string | null;
  /** The model the agent's chat runs on, beside its name while the title fits uncut beside both. */
  model?: string;
  /** The linked host the agent runs on, tagged before its name as the Mac's rail tags it. */
  host?: string;
  title?: string;
  /** Quiet words after the title on its line, such as what a chief's chat is on. */
  aside?: string;
  detail?: ReactNode;
  /** The project the row belongs to, marked with the folder the Mac's rail puts beside it. */
  place?: string | null;
  end?: ReactNode;
  onPress?: () => void;
  onLongPress?: () => void;
  dim?: boolean;
  detailLines?: number;
  /** One more line below the detail. */
  under?: ReactNode;
  /** A control of its own at the row's end, beside the row's press rather than inside it, as nested presses misbehave on iOS. */
  after?: ReactNode;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const handle = name ? `@${name}` : undefined;
  const label = title ?? handle;
  const named = title ? handle : undefined;
  const measured = named && model ? ` · ${model}` : undefined;
  const screen = useWindowDimensions();
  const scale = `${screen.width}x${screen.fontScale}`;
  const titleOf = `${scale}|${label}|${aside ?? ''}`;
  const modelOf = `${scale}|${measured}`;
  const [fit, setFit] = useState<Fit>({ title: unmeasured, model: unmeasured, body: { ...unmeasured, withModel: false } });
  // The model stays out until the row is measured as it is now, so it never shows and then drops.
  const modelShown =
    measured && fit.title.of === titleOf && fit.model.of === modelOf && fit.body.of === scale
      ? fit.title.width + fit.model.width <= fit.body.width + (fit.body.withModel ? fit.model.width : 0)
      : false;
  const setWidth = (part: 'title' | 'model', of: string, width: number) =>
    setFit((was) => (was[part].of === of && was[part].width === width ? was : { ...was, [part]: { of, width } }));
  const row = (
    <Pressable
      onPress={onPress}
      onLongPress={onLongPress}
      accessibilityRole={onPress ? 'button' : undefined}
      style={({ pressed }) => [
        styles.row,
        !place && !detail && !under && styles.rowOneLine,
        after ? styles.rowBeside : null,
        pressed && onPress && styles.rowPressed,
      ]}>
      {mark ? <View style={styles.mark}>{mark}</View> : null}
      {measured ? (
        <View style={styles.rowMeasure} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
          <View key={titleOf} style={styles.rowLabel} onLayout={(e) => setWidth('title', titleOf, e.nativeEvent.layout.width)}>
            <Text style={styles.rowTitle}>{label}</Text>
            {aside ? <Text style={styles.rowAside}>{aside}</Text> : null}
          </View>
          <Text
            key={modelOf}
            style={styles.rowName}
            numberOfLines={1}
            onLayout={(e) => setWidth('model', modelOf, e.nativeEvent.layout.width)}>
            {measured}
          </Text>
        </View>
      ) : null}
      <View
        key={scale}
        style={styles.rowBody}
        onLayout={
          measured
            ? (e) => {
                const width = e.nativeEvent.layout.width;
                const { body } = fit;
                if (body.of === scale && body.width === width && body.withModel === modelShown) return;
                setFit((was) => ({ ...was, body: { of: scale, width, withModel: modelShown } }));
              }
            : undefined
        }>
        <View style={styles.rowLabel}>
          <Text style={[styles.rowTitle, { color: dim ? colors.tertiary : colors.ink }]} numberOfLines={1}>
            {label}
          </Text>
          {aside ? (
            <Text style={styles.rowAside} numberOfLines={1}>
              {aside}
            </Text>
          ) : null}
        </View>
        {place ? (
          <View style={styles.rowPlace}>
            <Icon name="IconFolder" size={11} color={colors.live} />
            <Text style={[styles.rowDetail, styles.rowPlaceText]} numberOfLines={1}>
              {place}
              {detail ? <> · {detail}</> : null}
            </Text>
          </View>
        ) : detail ? (
          <Text style={styles.rowDetail} numberOfLines={detailLines}>
            {detail}
          </Text>
        ) : null}
        {under ? (
          <Text style={styles.rowDetail} numberOfLines={1}>
            {under}
          </Text>
        ) : null}
      </View>
      {named || host || end ? (
        <View style={styles.rowEnd}>
          {host ? (
            <View style={[styles.rowHost, dim && styles.rowHostDim]}>
              <Icon name="IconServer" size={11} color={colors.live} />
              <Text style={styles.rowHostName} numberOfLines={1}>
                {host}
              </Text>
            </View>
          ) : null}
          {named ? (
            <View style={styles.rowNamed}>
              <Text style={styles.rowName} numberOfLines={1}>
                {named}
              </Text>
              {modelShown ? (
                <Text style={[styles.rowName, styles.rowModel]} numberOfLines={1}>
                  {measured}
                </Text>
              ) : null}
            </View>
          ) : null}
          {end}
        </View>
      ) : null}
    </Pressable>
  );
  if (!after) return row;
  return (
    <View style={styles.rowWith}>
      {row}
      {after}
    </View>
  );
}

const ROW_INSET = 8;
const MARK = 20;
const ROW_GAP = 2;

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    sectionLabel: { flexDirection: 'row', alignItems: 'center', gap: 10, paddingTop: 18, paddingBottom: 6, paddingHorizontal: 8 },
    sectionLabelText: { fontFamily: fonts.uiMedium, fontSize: 11, letterSpacing: 1.76, textTransform: 'uppercase', color: colors.tertiary },
    sectionCount: { marginLeft: -4, fontFamily: fonts.uiMedium, fontSize: 11, color: colors.inkDim, fontVariant: ['tabular-nums'] },
    sectionRule: { flex: 1, height: 1, backgroundColor: translucent(colors.ink, 0.09) },
    rows: { gap: ROW_GAP },
    row: {
      minHeight: 48,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      paddingHorizontal: ROW_INSET,
      paddingVertical: 6,
      borderRadius: 9,
    },
    rowOneLine: { minHeight: 36 },
    rowPressed: { backgroundColor: colors.active },
    rowWith: { flexDirection: 'row', alignItems: 'center', paddingRight: ROW_INSET },
    rowBeside: { flex: 1, minWidth: 0 },
    mark: { width: MARK, height: MARK, alignItems: 'center', justifyContent: 'center' },
    rowBody: { flex: 1, minWidth: 0 },
    rowLabel: { flexDirection: 'row', alignItems: 'baseline' },
    rowTitle: { ...type.row, fontSize: 14.5, flexShrink: 1 },
    rowHost: { maxWidth: 150, flexShrink: 1, flexDirection: 'row', alignItems: 'center', gap: 4 },
    rowHostDim: { opacity: 0.55 },
    rowHostName: { flexShrink: 1, fontFamily: fonts.uiMedium, fontSize: 11, color: colors.live },
    rowAside: { flexShrink: 1, marginLeft: 6, fontFamily: fonts.ui, fontSize: 14.5, color: colors.tertiary },
    rowNamed: { flexShrink: 1, minWidth: 0, flexDirection: 'row', alignItems: 'baseline' },
    rowMeasure: { position: 'absolute', top: 0, left: 0, width: 10_000, flexDirection: 'row', alignItems: 'flex-start', opacity: 0 },
    rowModel: { flexShrink: 1 },
    rowName: { maxWidth: 110, fontFamily: fonts.ui, fontSize: 11.5, color: colors.tertiary },
    rowDetail: { ...type.meta, fontSize: 12, marginTop: 1 },
    rowPlace: { flexDirection: 'row', alignItems: 'center', gap: 4, marginTop: 1 },
    rowPlaceText: { flexShrink: 1, marginTop: 0 },
    rowEnd: { flexShrink: 1, minWidth: 20, alignItems: 'center', justifyContent: 'center', flexDirection: 'row', gap: 6 },
  });
};
