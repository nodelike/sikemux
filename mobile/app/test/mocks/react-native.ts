type Listener = (state: string) => void;

/** Outlives `vi.resetModules`, so a test can move the app that a freshly imported module listens to. */
const shared = globalThis as { sikemuxAppState?: Set<Listener> };
const appStateListeners = (shared.sikemuxAppState ??= new Set<Listener>());

export const Platform = {
  OS: 'ios' as 'ios' | 'android',
  select: <T>(choices: { ios?: T; android?: T; default?: T }): T | undefined => choices.ios ?? choices.default,
};

export const AppState = {
  currentState: 'active',
  addEventListener(_: 'change', listener: Listener) {
    appStateListeners.add(listener);
    return { remove: () => appStateListeners.delete(listener) };
  },
  /** Moves the fake app to the background or foreground, as the phone would. */
  emit(state: string) {
    AppState.currentState = state;
    for (const listener of appStateListeners) listener(state);
  },
};

export const StyleSheet = {
  create: <T>(styles: T): T => styles,
  absoluteFill: { position: 'absolute', top: 0, right: 0, bottom: 0, left: 0 },
  hairlineWidth: 0.5,
};
