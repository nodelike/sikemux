/*
 * Stands in for the Mac app's store when its chat view runs in a browser. The view reads which
 * plugins are switched off and whether buddies are on; no plugin runs and buddies stay off here.
 */
const state = {
  disabledPlugins: [] as string[],
  pluginManifests: [] as unknown[],
  buddies: false,
  agents: {} as Record<string, never>,
  agentActivity: {} as Record<string, never>,
};

export function getState() {
  return state;
}

export function useStore<T>(select: (current: typeof state) => T): T {
  return select(state);
}

export function mutate(): void {}
