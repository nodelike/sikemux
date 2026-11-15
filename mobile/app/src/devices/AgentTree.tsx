import type { ReactNode } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import type { Line } from '@/devices/tree';
import { toggleFolded } from '@/devices/folded';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/** Down the middle of a root row's mark, which sits 8 into its row, and how far each level sits right of the one above. */
const SPINE = 18;
const INDENT = 22;

/** The fold key of the team under an agent. */
export const teamFold = (agentId: string) => `team:${agentId}`;

/** The spines, ticks and corners that hang a row under its lead, and the line dropping from its mark to its team. */
export function TreeLines({
  depth = 0,
  lines = [],
  open = false,
  children,
}: {
  depth?: number;
  lines?: readonly Line[];
  open?: boolean;
  children: ReactNode;
}) {
  const styles = useStyles(makeStyles);
  const at = (level: number) => ({ left: SPINE + INDENT * level });
  return (
    <View style={depth ? { paddingLeft: INDENT * depth } : undefined}>
      {lines.map((line, level) =>
        line === 'blank' ? null : line === 'corner' ? (
          <View key={level} style={[styles.corner, at(level)]} />
        ) : (
          <View key={level} style={[styles.spine, styles.through, at(level)]}>
            {line === 'tick' ? <View style={styles.tick} /> : null}
          </View>
        ),
      )}
      {open ? (
        <View style={[styles.drop, at(depth)]}>
          <View style={styles.dropLine} />
        </View>
      ) : null}
      {children}
    </View>
  );
}

/** The chevron that folds the team under a row, with how many it hides while folded, as the Mac's rail counts them. */
export function TreeFold({ agentId, count, open }: { agentId: string; count: number; open: boolean }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <Pressable
      onPress={() => toggleFolded(teamFold(agentId))}
      hitSlop={8}
      accessibilityRole="button"
      accessibilityState={{ expanded: open }}
      accessibilityLabel={`+${count}`}
      style={styles.fold}>
      {open ? null : <Text style={styles.count}>+{count}</Text>}
      <View style={{ transform: [{ rotate: open ? '90deg' : '0deg' }] }}>
        <Icon name="IconChevron" size={12} color={colors.tertiary} />
      </View>
    </Pressable>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    spine: { position: 'absolute', width: 1, backgroundColor: colors.treeSpine },
    through: { top: -2, bottom: -2 },
    tick: { position: 'absolute', top: '50%', left: 0, width: 8, height: 1, backgroundColor: colors.treeSpine },
    corner: {
      position: 'absolute',
      top: -2,
      bottom: '50%',
      width: 8,
      borderLeftWidth: 1,
      borderBottomWidth: 1,
      borderColor: colors.treeSpine,
      borderBottomLeftRadius: 4,
    },
    drop: { position: 'absolute', top: '50%', bottom: -2, width: 1, paddingTop: 12 },
    dropLine: { flex: 1, backgroundColor: colors.treeSpine },
    fold: { flexDirection: 'row', alignItems: 'center', gap: 4, paddingLeft: 6, minHeight: 36 },
    count: {
      fontFamily: fonts.uiMedium,
      fontSize: 11,
      color: colors.tertiary,
      fontVariant: ['tabular-nums'],
      paddingHorizontal: 5,
      borderRadius: 8,
      overflow: 'hidden',
      backgroundColor: colors.active,
    },
  });
