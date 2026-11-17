import { useEffect, useRef, useState, type RefObject } from 'react';
import type { LayoutChangeEvent, NativeScrollEvent, NativeSyntheticEvent } from 'react-native';
import type { FlashListRef } from '@shopify/flash-list';

/** How near the end, as a share of the transcript's height, still counts as reading the latest. */
const FOLLOW = 0.1;
/** How many screens above the first message shown the turns before it are fetched, so a fling up never meets the top. */
const EARLIER_SCREENS = 3;

type Earlier = { has: boolean; idle: boolean; load: () => void };

/**
 * How a transcript scrolls: it stays at its end while the reader is there, whatever grows or shrinks it,
 * shows the way back once they scroll away, and fetches earlier turns well before they reach the top.
 */
export function useTranscriptScroll<T>(list: RefObject<FlashListRef<T> | null>, earlier: Earlier, holding = false) {
  const [away, setAway] = useState(false);
  const awayRef = useRef(false);
  const metrics = useRef({ offset: 0, content: 0, viewport: 0 });
  // Only the reader's own scrolling takes them away from the end.
  const touching = useRef(false);
  // While the reader works on one row, such as one being edited, growth elsewhere leaves the scroll alone.
  const held = useRef(holding);
  useEffect(() => {
    held.current = holding;
  }, [holding]);

  const measureAway = () => {
    const { offset, content, viewport } = metrics.current;
    const now = content - offset - viewport > viewport * FOLLOW;
    awayRef.current = now;
    setAway(now);
  };
  const remember = ({ nativeEvent }: NativeSyntheticEvent<NativeScrollEvent>) => {
    metrics.current = {
      offset: nativeEvent.contentOffset.y,
      content: nativeEvent.contentSize.height,
      viewport: nativeEvent.layoutMeasurement.height,
    };
  };
  const fetchEarlier = () => {
    const { offset, viewport } = metrics.current;
    if (earlier.has && earlier.idle && viewport > 0 && offset < viewport * EARLIER_SCREENS) earlier.load();
  };
  // The answer being written grows smoothly itself, so the end is followed exactly, keeping the activity line still.
  const keepAtEnd = () => {
    if (awayRef.current) return measureAway();
    if (touching.current || held.current) return;
    list.current?.scrollToEnd({ animated: false });
  };
  const startTouching = () => {
    touching.current = true;
  };
  // Throttled scroll events can miss where a drag or fling stops, so its end is measured too.
  const stopTouching = (event: NativeSyntheticEvent<NativeScrollEvent>) => {
    remember(event);
    measureAway();
    touching.current = false;
  };

  const toEnd = () => {
    awayRef.current = false;
    setAway(false);
    requestAnimationFrame(() => list.current?.scrollToEnd({ animated: true }));
  };

  /** Stops following the end, so rows growing below the top of the view leave what the reader sees where it is. */
  const hold = () => {
    awayRef.current = true;
    setAway(true);
  };

  /** Brings row `index` into view, `at` 0 at the top of the transcript and 1 at its bottom. */
  const reveal = (index: number, at: number) => {
    requestAnimationFrame(() => list.current?.scrollToIndex({ index, animated: true, viewPosition: at }));
  };

  return {
    away,
    toEnd,
    hold,
    reveal,
    listProps: {
      maintainVisibleContentPosition: { startRenderingFromBottom: true },
      scrollEventThrottle: 16,
      onScroll: (event: NativeSyntheticEvent<NativeScrollEvent>) => {
        remember(event);
        if (touching.current || awayRef.current) measureAway();
        fetchEarlier();
      },
      onScrollBeginDrag: startTouching,
      onMomentumScrollBegin: startTouching,
      onScrollEndDrag: stopTouching,
      onMomentumScrollEnd: stopTouching,
      onContentSizeChange: (_width: number, height: number) => {
        metrics.current.content = height;
        keepAtEnd();
      },
      onLayout: ({ nativeEvent }: LayoutChangeEvent) => {
        metrics.current.viewport = nativeEvent.layout.height;
        keepAtEnd();
      },
    },
  };
}
