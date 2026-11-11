import type { ReactNode } from 'react';
import { StyleSheet, Text, View, type StyleProp, type ViewStyle } from 'react-native';

import { clockLabel } from '@shared/chat/messageMeta';
import { fonts, mixed, type Palette, translucent, useStyles } from '@/ui/theme';

export const BUBBLE_RADIUS = 16;
const TAIL_RADIUS = 6;

/** The person's bubble: no border, its tail at the bottom right. */
export function Bubble({ style, children }: { style?: StyleProp<ViewStyle>; children: ReactNode }) {
  const styles = useStyles(makeBubbleStyles);
  return <View style={[styles.mine, style]}>{children}</View>;
}

/** What someone else said to the person: a bordered bubble with its tail at the bottom left. */
export function TheirBubble({ children }: { children: ReactNode }) {
  const styles = useStyles(makeBubbleStyles);
  return <View style={styles.theirs}>{children}</View>;
}

/** Who sent a bubble at the left, over it, with when at the line's end. */
export function From({ who, at, children }: { who: ReactNode; at?: number | null; children: ReactNode }) {
  const styles = useStyles(makeBubbleStyles);
  return (
    <View style={styles.from}>
      <View style={styles.fromHead}>
        {who}
        {at ? <Text style={styles.fromWhen}>{clockLabel(at)}</Text> : null}
      </View>
      <TheirBubble>{children}</TheirBubble>
    </View>
  );
}

export const makeBubbleStyles = (colors: Palette) => {
  return StyleSheet.create({
    mine: {
      maxWidth: '100%',
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: BUBBLE_RADIUS,
      borderBottomRightRadius: TAIL_RADIUS,
      backgroundColor: bubbleFill(colors),
    },
    theirs: {
      flexShrink: 1,
      maxWidth: '100%',
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: BUBBLE_RADIUS,
      borderBottomLeftRadius: TAIL_RADIUS,
      borderWidth: 1,
      borderColor: translucent(colors.border, 0.8),
      backgroundColor: colors.composer,
    },
    text: { fontFamily: fonts.ui, fontSize: 14, lineHeight: 22, color: colors.ink },
    from: { maxWidth: '86%', alignItems: 'flex-start', gap: 6 },
    fromHead: { alignSelf: 'stretch', flexDirection: 'row', alignItems: 'center', gap: 8, minHeight: 22 },
    fromWhen: { marginLeft: 'auto', paddingLeft: 8, fontFamily: fonts.ui, fontSize: 11.5, color: colors.inkFaint },
  });
};

/** The person's bubble: the accent warmed toward the ink, faintly over the ground, as the Mac fills it. */
export function bubbleFill(colors: Palette): string {
  return translucent(mixed(colors.accent, colors.ink, 0.25), 0.075);
}
