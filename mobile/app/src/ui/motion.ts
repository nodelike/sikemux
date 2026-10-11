import { useEffect, useRef, useSyncExternalStore } from 'react';
import { AccessibilityInfo, AppState } from 'react-native';
import { useIsFocused } from 'expo-router';

/** One system setting, followed once for the whole app however many components read it. */
function followed<T>(initial: T, follow: (set: (value: T) => void) => void) {
  let value = initial;
  let following = false;
  const listeners = new Set<() => void>();
  return {
    subscribe(listener: () => void) {
      if (!following) {
        following = true;
        follow((next) => {
          if (next === value) return;
          value = next;
          listeners.forEach((listen) => listen());
        });
      }
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    get: () => value,
  };
}

const reduceMotion = followed(false, (set) => {
  AccessibilityInfo.isReduceMotionEnabled()
    .then(set)
    .catch(() => {});
  AccessibilityInfo.addEventListener('reduceMotionChanged', set);
});

const appActive = followed(AppState.currentState === 'active', (set) => {
  AppState.addEventListener('change', (state) => set(state === 'active'));
});

/** Whether the person asked the system to reduce motion. */
export function useStill(): boolean {
  return useSyncExternalStore(reduceMotion.subscribe, reduceMotion.get);
}

let minute = Math.floor(Date.now() / 60_000);
const minuteListeners = new Set<() => void>();
let minuteTimer: ReturnType<typeof setInterval> | undefined;

/** The current minute, from one clock the whole app shares, for labels like "5m" that age as they sit. */
export function useMinute(): number {
  return useSyncExternalStore(
    (listener) => {
      minuteListeners.add(listener);
      minuteTimer ??= setInterval(() => {
        minute = Math.floor(Date.now() / 60_000);
        minuteListeners.forEach((each) => each());
      }, 60_000);
      return () => {
        minuteListeners.delete(listener);
        if (minuteListeners.size === 0) {
          clearInterval(minuteTimer);
          minuteTimer = undefined;
        }
      };
    },
    () => minute,
  );
}

/** Whether the app is in front. */
export function useAppActive(): boolean {
  return useSyncExternalStore(appActive.subscribe, appActive.get);
}

/**
 * Whether a looping animation can be seen: motion is allowed, the app is in front and its screen is the one
 * showing. A native-driven loop runs every frame on the UI thread even under another screen, so loops hold still
 * otherwise.
 */
export function useAnimating(): boolean {
  const still = useStill();
  const active = useAppActive();
  const focused = useIsFocused();
  return !still && active && focused;
}

let pauses = 0;
const pauseListeners = new Set<() => void>();

function pausesChanged(by: number) {
  pauses += by;
  pauseListeners.forEach((listen) => listen());
}

/** Stills the backdrop until the returned function is called: a sheet covers it, or the screen is scrolling. */
export function pauseBackdrop(): () => void {
  pausesChanged(1);
  let released = false;
  return () => {
    if (released) return;
    released = true;
    pausesChanged(-1);
  };
}

function subscribePauses(listener: () => void) {
  pauseListeners.add(listener);
  return () => {
    pauseListeners.delete(listener);
  };
}

export function useBackdropPaused(): boolean {
  return useSyncExternalStore(subscribePauses, () => pauses > 0);
}

/** Scroll handlers that still the backdrop while a list moves, so scrolling keeps the JavaScript thread free. */
export function useScrollPause() {
  const held = useRef<() => void>(undefined);
  useEffect(() => () => held.current?.(), []);
  const hold = () => {
    held.current ??= pauseBackdrop();
  };
  const letGo = () => {
    held.current?.();
    held.current = undefined;
  };
  return { onScrollBeginDrag: hold, onMomentumScrollBegin: hold, onScrollEndDrag: letGo, onMomentumScrollEnd: letGo };
}
