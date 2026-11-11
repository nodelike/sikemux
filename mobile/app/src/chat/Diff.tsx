import { memo, useMemo, type ReactNode } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { collapseDiff } from '@shared/chat/diff';
import type { DiffLine } from '@shared/wire.generated';
import { counted } from '@/devices/words';
import { Icon } from '@/ui/Icon';
import { fonts, mixed, type Palette, translucent, useColors, useStyles } from '@/ui/theme';
import { Sideways } from '@/ui/Sideways';
import { useFold } from './folds';

/** Unchanged lines kept around each change, as on the Mac. */
const CONTEXT = 3;
/** Rows drawn before the rest waits for a tap, so opening a whole new file costs one screenful. */
const FIRST_ROWS = 120;

/** Scrolls sideways, so long lines keep to the width of the screen and every row's tint spans the longest. */
export function SidewaysRows({ children }: { children: ReactNode }) {
  const styles = useStyles(makeStyles);
  return (
    <Sideways contentStyle={styles.sideways} style={styles.rows}>
      {children}
    </Sideways>
  );
}

export const DiffRow = memo(function DiffRow({ line, numbered }: { line: DiffLine; numbered: boolean }) {
  const styles = useStyles(makeStyles);
  const added = line.sign === '+';
  const deleted = line.sign === '-';
  const mark = line.mark;
  const numbers = [styles.number, added && styles.addedNumber, deleted && styles.deletedNumber];
  return (
    <View style={[styles.line, added && styles.added, deleted && styles.deleted, !numbered && styles.unnumbered]}>
      {numbered ? (
        <>
          <Text style={[styles.mono, numbers]}>{line.oldLine}</Text>
          <Text style={[styles.mono, numbers]}>{line.newLine}</Text>
        </>
      ) : null}
      <Text style={[styles.mono, styles.sign, added && styles.addedSign, deleted && styles.deletedSign]}>
        {line.sign === ' ' ? '' : line.sign === '-' ? '−' : '+'}
      </Text>
      <Text style={[styles.mono, styles.text, added && styles.addedInk, deleted && styles.deletedInk]} selectable>
        {mark ? (
          <>
            {line.text.slice(0, mark[0])}
            <Text style={deleted ? styles.deletedMark : styles.addedMark}>{line.text.slice(mark[0], mark[1])}</Text>
            {line.text.slice(mark[1])}
          </>
        ) : (
          line.text
        )}
      </Text>
    </View>
  );
});

const NO_LINES: DiffLine[] = [];

/**
 * What an edit changed, as the Mac draws it under the call: old and new line numbers, a +/− gutter, tinted text and the
 * unchanged runs folded. `waiting` stands in for the lines until the host has sent them.
 */
export function DiffBody({ lines = NO_LINES, fold, waiting }: { lines?: DiffLine[]; fold: string; waiting?: string }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const [expanded, setExpanded] = useFold(`${fold}/context`, false);
  const [whole, setWhole] = useFold(`${fold}/rows`, false);
  const rows = useMemo(() => collapseDiff(lines, expanded ? Number.MAX_SAFE_INTEGER : CONTEXT).rows, [lines, expanded]);
  const shown = whole ? rows : rows.slice(0, FIRST_ROWS);
  const more = rows.length - shown.length;
  return (
    <View style={styles.box}>
      {waiting ? <Text style={styles.waiting}>{waiting}</Text> : null}
      <SidewaysRows>
        {shown.map((row, index) =>
          'gap' in row ? (
            <Pressable key={`gap-${index}`} onPress={() => setExpanded(true)} style={styles.gap} accessibilityRole="button">
              <Icon name="IconChevron" size={9} color={colors.inkDim} />
              <Text style={styles.gapText}>
                {row.gap} more {counted('line', row.gap)}
              </Text>
            </Pressable>
          ) : (
            <DiffRow key={index} line={row} numbered />
          ),
        )}
      </SidewaysRows>
      {more > 0 ? (
        <Pressable onPress={() => setWhole(true)} style={styles.more} accessibilityRole="button">
          <Text style={styles.gapText}>Show {more} more lines</Text>
        </Pressable>
      ) : null}
    </View>
  );
}

/** The two number columns and the sign's, before a diff line's text, as on the Mac but narrower. */
const NUMBER_WIDTH = 28;
const SIGN_WIDTH = 14;

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    box: { borderRadius: 8, backgroundColor: colors.sunken, overflow: 'hidden' },
    mono: { fontFamily: fonts.mono, fontSize: 11, lineHeight: 20 },
    sideways: { flexGrow: 1, paddingVertical: 4 },
    rows: { flexGrow: 1 },
    line: { flexDirection: 'row', paddingRight: 10 },
    unnumbered: { paddingLeft: 4 },
    added: { backgroundColor: translucent(colors.gitAdded, 0.09) },
    deleted: { backgroundColor: translucent(colors.gitDeleted, 0.09) },
    number: {
      width: NUMBER_WIDTH,
      paddingRight: 6,
      fontSize: 10,
      textAlign: 'right',
      color: mixed(colors.inkFaint, colors.sunken, 0.5),
    },
    addedNumber: { color: mixed(colors.gitAdded, colors.sunken, 0.38) },
    deletedNumber: { color: mixed(colors.gitDeleted, colors.sunken, 0.38) },
    sign: { width: SIGN_WIDTH, textAlign: 'center', color: colors.inkFaint },
    addedSign: { color: colors.gitAdded },
    deletedSign: { color: colors.gitDeleted },
    text: { color: colors.inkDim },
    addedInk: { color: mixed(colors.gitAdded, colors.ink, 0.22) },
    deletedInk: { color: mixed(colors.gitDeleted, colors.ink, 0.25) },
    addedMark: { backgroundColor: translucent(colors.gitAdded, 0.26) },
    deletedMark: { backgroundColor: translucent(colors.gitDeleted, 0.26) },
    gap: { flexDirection: 'row', alignItems: 'center', gap: 6, height: 26, paddingLeft: NUMBER_WIDTH * 2 + SIGN_WIDTH },
    gapText: { fontFamily: fonts.ui, fontSize: 11, color: colors.inkDim },
    waiting: { paddingHorizontal: 10, paddingVertical: 7, fontFamily: fonts.mono, fontSize: 10.5, color: colors.inkFaint },
    more: { paddingHorizontal: 10, paddingVertical: 7 },
  });
