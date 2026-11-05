import { useEffect, useRef } from 'react';
import { AccessibilityInfo, AppState } from 'react-native';
import { useIsFocused } from 'expo-router';

import { externalValue, useExternalValue, type ExternalValue } from '@shared/lib/externalValue';

const reduceMotion: ExternalValue<boolean> = externalValue(false, () => {
  AccessibilityInfo.isReduceMotionEnabled()
    .then(reduceMotion.set)
    .catch((error: unknown) => console.warn('sikemux: could not read whether to reduce motion', error));
  const following = AccessibilityInfo.addEventListener('reduceMotionChanged', reduceMotion.set);
  return () => following.remove();
});

const appActive: ExternalValue<boolean> = externalValue(AppState.currentState === 'active', () => {
  appActive.set(AppState.currentState === 'active');
  const following = AppState.addEventListener('change', (state) => appActive.set(state === 'active'));
  return () => following.remove();
});

/** Whether the person asked the system to reduce motion. */
export function useStill(): boolean {
  return useExternalValue(reduceMotion);
}

const thisMinute = () => Math.floor(Date.now() / 60_000);

const minute: ExternalValue<number> = externalValue(thisMinute(), () => {
  minute.set(thisMinute());
  const ticking = setInterval(() => minute.set(thisMinute()), 60_000);
  return () => clearInterval(ticking);
});

/** The current minute, from one clock the whole app shares, for labels like "5m" that age as they sit. */
export function useMinute(): number {
  return useExternalValue(minute);
}

const thisSecond = () => Math.floor(Date.now() / 1000);

const second: ExternalValue<number> = externalValue(thisSecond(), () => {
  second.set(thisSecond());
  const ticking = setInterval(() => second.set(thisSecond()), 1000);
  return () => clearInterval(ticking);
});

/** The current second, from one clock every running row shares; it stands still while `on` is false. */
export function useSecond(on = true): number {
  return useExternalValue(second, (now) => (on ? now : 0));
}

/** Whether the app is in front. */
export function useAppActive(): boolean {
  return useExternalValue(appActive);
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

const pauses = externalValue(0);

function pausesChanged(by: number) {
  pauses.set(pauses.get() + by);
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

export function useBackdropPaused(): boolean {
  return useExternalValue(pauses, (count) => count > 0);
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
