import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from 'react';
import { Animated, Pressable, StyleSheet, Text, View } from 'react-native';

import { useT } from '@/i18n';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, translucent, useColors, useStyles } from '@/ui/theme';

/** Where a row sits in the list's content. */
type Place = { y: number; height: number };

type Marks = { content: RefObject<View | null>; report: (id: string, place?: Place) => void };

const MarksContext = createContext<Marks | null>(null);

/** Gives the list's rows where to say they sit. */
export const NeedsYouMarks = MarksContext.Provider;

/** Keeps where `children` sits in the list while `on`, so the list's edge can point to it once it is scrolled away. */
export function NeedsYouMark({ id, on, children }: { id: string; on: boolean; children: ReactNode }) {
  const marks = useContext(MarksContext);
  const row = useRef<View>(null);
  const measure = () => {
    const content = marks?.content.current;
    if (!on || !content || !row.current) return;
    row.current.measureLayout(content, (_x, y, _width, height) => marks.report(id, { y, height }));
  };
  useEffect(() => {
    if (on) measure();
    else marks?.report(id);
    return () => marks?.report(id);
  }, [id, on, marks]);
  return (
    <View ref={row} onLayout={measure} collapsable={false}>
      {children}
    </View>
  );
}

/** What the list keeps out of sight above and below the part on screen, nearest first. */
type Unseen = { above: string[]; below: string[] };

/**
 * The rows that need the person but are scrolled out of the list, with a chip at the list's top and bottom edges that
 * scrolls the nearest of them into sight, as the Mac's rail does.
 */
export function useNeedsYou(scrollTo: (y: number) => void) {
  const content = useRef<View>(null);
  const places = useRef(new Map<string, Place>());
  const seen = useRef({ offset: 0, top: 0, bottom: 0 });
  const [unseen, setUnseen] = useState<Unseen>({ above: [], below: [] });

  const sort = useCallback(() => {
    const { offset, top, bottom } = seen.current;
    if (bottom <= top) return;
    const above: [string, number][] = [];
    const below: [string, number][] = [];
    for (const [id, place] of places.current) {
      if (place.y + place.height <= offset + top) above.push([id, -place.y]);
      else if (place.y >= offset + bottom) below.push([id, place.y]);
    }
    const next = { above: above.sort((a, b) => a[1] - b[1]).map(([id]) => id), below: below.sort((a, b) => a[1] - b[1]).map(([id]) => id) };
    setUnseen((now) => (now.above.join() === next.above.join() && now.below.join() === next.below.join() ? now : next));
  }, []);

  const marks = useMemo<Marks>(
    () => ({
      content,
      report: (id, place) => {
        if (place) places.current.set(id, place);
        else places.current.delete(id);
        sort();
      },
    }),
    [sort],
  );

  /** The scroll offset, and where the list's visible part starts and ends on screen. */
  const view = useCallback(
    (offset: number, top: number, bottom: number) => {
      seen.current = { offset, top, bottom };
      sort();
    },
    [sort],
  );

  const bring = (id: string | undefined) => {
    const place = id ? places.current.get(id) : undefined;
    if (!place) return;
    const { top, bottom } = seen.current;
    haptics.select();
    scrollTo(Math.max(0, place.y + place.height / 2 - (top + bottom) / 2));
  };

  return { content, marks, view, unseen, onAbove: () => bring(unseen.above[0]), onBelow: () => bring(unseen.below[0]) };
}

/** The chip at an edge of the list, counting the rows past it that need the person. */
export function NeedsYouChip({ count, up, onPress, style }: { count: number; up: boolean; onPress: () => void; style?: object }) {
  const styles = useStyles(makeStyles);
  const colors = useColors();
  const t = useT();
  if (!count) return null;
  return (
    <Animated.View style={[styles.at, style]} pointerEvents="box-none">
      <Pressable onPress={onPress} accessibilityRole="button" accessibilityLabel={t('Needs you')} style={styles.chip}>
        <View style={styles.fill}>
          <Icon name={up ? 'IconArrowUp' : 'IconArrowDown'} size={11} color={colors.warn} />
          <Text style={styles.text}>{t('Needs you')}</Text>
          {count > 1 ? <Text style={[styles.text, styles.count]}>{count}</Text> : null}
        </View>
      </Pressable>
    </Animated.View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    at: { position: 'absolute', left: 0, right: 0, alignItems: 'center' },
    chip: {
      borderRadius: 12,
      backgroundColor: colors.raised,
      shadowColor: '#000',
      shadowOpacity: 0.3,
      shadowRadius: 6,
      shadowOffset: { width: 0, height: 2 },
      elevation: 3,
    },
    fill: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 4,
      height: 24,
      paddingHorizontal: 10,
      borderRadius: 12,
      backgroundColor: translucent(colors.warn, 0.2),
    },
    text: { fontFamily: fonts.uiMedium, fontSize: 12, color: colors.warn },
    count: { fontVariant: ['tabular-nums'] },
  });
