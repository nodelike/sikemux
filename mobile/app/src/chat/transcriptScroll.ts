import { useEffect, useRef, useState, type RefObject } from 'react';
import { Keyboard, Platform, type LayoutChangeEvent, type NativeScrollEvent, type NativeSyntheticEvent } from 'react-native';
import type { FlashListRef } from '@shopify/flash-list';

/** How near the end, as a share of the transcript's height, still counts as reading the latest. */
const FOLLOW = 0.1;
/** How many screens above the first message shown the turns before it are fetched, so a fling up never meets the top. */
const EARLIER_SCREENS = 3;
/** How long the glide takes to close whatever gap is left between it and the chat's end. */
const CLOSE_MS = 250;
/** Over how long the glide averages how fast the chat grows, so a new line does not jolt it. */
const AVERAGE_MS = 400;

type Earlier = { has: boolean; idle: boolean; load: () => void };

/**
 * How a transcript scrolls: it stays at its end while the reader is there, whatever grows or shrinks it,
 * shows the way back once they scroll away, and fetches earlier turns well before they reach the top.
 */
export function useTranscriptScroll<T>(list: RefObject<FlashListRef<T> | null>, earlier: Earlier) {
  const [away, setAway] = useState(false);
  const awayRef = useRef(false);
  const metrics = useRef({ offset: 0, content: 0, viewport: 0 });
  // Only the reader's own scrolling takes them away from the end.
  const touching = useRef(false);
  const gliding = useRef<number | null>(null);

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
  const stopGliding = () => {
    if (gliding.current !== null) cancelAnimationFrame(gliding.current);
    gliding.current = null;
  };
  // Text grows a line at a time; the chat moves up at the pace it grows on average, rather than a line at a time.
  const glide = () => {
    let at = metrics.current.offset;
    let end = Math.max(0, metrics.current.content - metrics.current.viewport);
    let pace = 0;
    let last = 0;
    gliding.current = requestAnimationFrame(function step(now) {
      const since = last ? Math.min(now - last, 50) : 16;
      last = now;
      const next = Math.max(0, metrics.current.content - metrics.current.viewport);
      pace += ((Math.max(0, next - end) / since - pace) * since) / AVERAGE_MS;
      end = next;
      at = Math.min(end, at + (pace + (end - at) / CLOSE_MS) * since);
      const there = end - at < 0.5 && pace < 0.01;
      list.current?.scrollToOffset({ offset: there ? end : at, animated: false });
      gliding.current = there ? null : requestAnimationFrame(step);
    });
  };
  const keepAtEnd = () => {
    if (awayRef.current) return measureAway();
    if (touching.current) return;
    const { offset, content, viewport } = metrics.current;
    const end = content - viewport;
    // A shrinking chat, or one that grew by more than a screen at once, such as on opening, goes straight to its end.
    if (end <= offset || end - offset > viewport) {
      stopGliding();
      list.current?.scrollToEnd({ animated: false });
    } else if (gliding.current === null) glide();
  };
  const startTouching = () => {
    touching.current = true;
    stopGliding();
  };
  // Throttled scroll events can miss where a drag or fling stops, so its end is measured too.
  const stopTouching = (event: NativeSyntheticEvent<NativeScrollEvent>) => {
    remember(event);
    measureAway();
    touching.current = false;
  };

  useEffect(() => {
    // The keyboard takes the bottom of the screen; someone reading the latest keeps reading it.
    const shown = Keyboard.addListener(Platform.OS === 'ios' ? 'keyboardWillShow' : 'keyboardDidShow', () => {
      if (!awayRef.current) requestAnimationFrame(() => list.current?.scrollToEnd({ animated: true }));
    });
    return () => {
      shown.remove();
      if (gliding.current !== null) cancelAnimationFrame(gliding.current);
    };
  }, [list]);

  const toEnd = () => {
    awayRef.current = false;
    setAway(false);
    requestAnimationFrame(() => list.current?.scrollToEnd({ animated: true }));
  };

  return {
    away,
    toEnd,
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
