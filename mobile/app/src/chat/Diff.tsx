import { memo, useMemo, type ReactNode } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { collapseDiff } from '@mac/chat/diff';
import type { DiffLine, ToolDiff } from '@mac/chat/types';
import { counted } from '@/devices/words';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, translucent, useColors, useStyles } from '@/ui/theme';
import { useFold } from './folds';

/** Unchanged lines kept around each change, as on the Mac. */
const CONTEXT = 3;
/** Rows drawn before the rest waits for a tap, so opening a whole new file costs one screenful. */
const FIRST_ROWS = 120;

/* While a view around a scroller holds the touch, iOS won't let the scroller pan, so the
   scroller's own content takes it before the message's press handler can. */
const keepTouch = () => true;

/** Scrolls sideways, so long lines keep to the width of the screen and every row's tint spans the longest. */
export function SidewaysRows({ children }: { children: ReactNode }) {
  const styles = useStyles(makeStyles);
  return (
    <ScrollView horizontal contentContainerStyle={styles.sideways} showsHorizontalScrollIndicator={false}>
      <View onStartShouldSetResponder={keepTouch} style={styles.rows}>
        {children}
      </View>
    </ScrollView>
  );
}

/** One line of a change: tinted by its sign, with the span that changed marked inside it. */
export const DiffRow = memo(function DiffRow({ line, numbered }: { line: DiffLine; numbered: boolean }) {
  const styles = useStyles(makeStyles);
  const added = line.sign === '+';
  const deleted = line.sign === '-';
  const mark = line.mark;
  return (
    <View style={[styles.line, added && styles.added, deleted && styles.deleted, !numbered && styles.unnumbered]}>
      {numbered ? <Text style={[styles.mono, styles.number]}>{added ? line.newLine : line.oldLine}</Text> : null}
      <Text style={[styles.mono, styles.sign, added && styles.addedInk, deleted && styles.deletedInk]}>{line.sign}</Text>
      <Text style={[styles.mono, styles.text]} selectable>
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
 * What an edit changed, as the Mac draws it under the call: the file, its counts, numbered lines and the unchanged runs
 * folded. `waiting` stands in for the lines until the host has sent them.
 */
export function DiffBody({
  diff,
  fold,
  waiting,
}: {
  diff: Omit<ToolDiff, 'lines'> & { lines?: DiffLine[] };
  fold: string;
  waiting?: string;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const [expanded, setExpanded] = useFold(`${fold}/context`, false);
  const [whole, setWhole] = useFold(`${fold}/rows`, false);
  const lines = diff.lines ?? NO_LINES;
  const rows = useMemo(() => collapseDiff(lines, expanded ? Number.MAX_SAFE_INTEGER : CONTEXT).rows, [lines, expanded]);
  const shown = whole ? rows : rows.slice(0, FIRST_ROWS);
  const more = rows.length - shown.length;
  return (
    <View style={styles.box}>
      <View style={styles.head}>
        <Icon name="IconFile" size={11} color={colors.inkFaint} />
        <Text style={[styles.mono, styles.path]} numberOfLines={1} ellipsizeMode="head">
          {diff.path}
        </Text>
        <Text style={[styles.mono, styles.count, styles.addedInk]}>+{diff.adds}</Text>
        <Text style={[styles.mono, styles.count, styles.deletedInk]}>−{diff.dels}</Text>
      </View>
      {waiting ? <Text style={[styles.mono, styles.waiting]}>{waiting}</Text> : null}
      <SidewaysRows>
        {shown.map((row, index) =>
          'gap' in row ? (
            <Pressable key={`gap-${index}`} onPress={() => setExpanded(true)} style={styles.gap} accessibilityRole="button">
              <Text style={[styles.mono, styles.gapText]}>
                {row.gap} unchanged {counted('line', row.gap)}
              </Text>
            </Pressable>
          ) : (
            <DiffRow key={index} line={row} numbered />
          ),
        )}
      </SidewaysRows>
      {more > 0 ? (
        <Pressable onPress={() => setWhole(true)} style={styles.more} accessibilityRole="button">
          <Text style={[styles.mono, styles.gapText]}>Show {more} more lines</Text>
        </Pressable>
      ) : null}
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    box: { borderRadius: 8, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.sunken, overflow: 'hidden' },
    head: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 7,
      height: 24,
      paddingHorizontal: 9,
      borderBottomWidth: 1,
      borderBottomColor: colors.border,
    },
    path: { flex: 1, fontSize: 10.5, color: colors.inkDim },
    count: { fontSize: 10.5 },
    mono: { fontFamily: fonts.mono, fontSize: 11, lineHeight: 17 },
    sideways: { flexGrow: 1, paddingVertical: 3 },
    rows: { flexGrow: 1 },
    line: { flexDirection: 'row', paddingRight: 12 },
    unnumbered: { paddingLeft: 10 },
    added: { backgroundColor: translucent(colors.gitAdded, 0.09) },
    deleted: { backgroundColor: translucent(colors.gitDeleted, 0.09) },
    number: { width: 30, paddingRight: 7, fontSize: 10, textAlign: 'right', color: translucent(colors.inkFaint, 0.58) },
    sign: { width: 12, color: translucent(colors.inkFaint, 0.7) },
    addedInk: { color: colors.gitAdded },
    deletedInk: { color: colors.gitDeleted },
    text: { color: colors.ink },
    addedMark: { backgroundColor: translucent(colors.gitAdded, 0.26) },
    deletedMark: { backgroundColor: translucent(colors.gitDeleted, 0.26) },
    gap: { height: 20, justifyContent: 'center', paddingLeft: 42 },
    gapText: { fontSize: 10.5, color: colors.inkFaint },
    waiting: { paddingHorizontal: 9, paddingVertical: 6, fontSize: 10.5, color: colors.inkFaint },
    more: { paddingHorizontal: 9, paddingVertical: 7, borderTopWidth: 1, borderTopColor: colors.border },
  });
