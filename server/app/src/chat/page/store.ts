import type { AgentFace, AgentShown, FaceStyle } from "@shared/wire.generated";
import type { Agent } from "./agent";

/*
 * What the chat view reads of the app's state: which plugins are switched off and whether faces
 * are on. No plugin runs on the page, and faces stay off.
 */
interface PageState {
  readonly disabledPlugins: readonly string[];
  readonly faceStyle: FaceStyle;
  readonly faces: Readonly<Record<string, AgentFace | null>>;
  readonly agents: Readonly<Record<string, Agent>>;
  readonly agentStatus: Readonly<Record<string, AgentShown>>;
}

const state: PageState = {
  disabledPlugins: [],
  faceStyle: { on: false, pack: "", shape: "", motion: false },
  faces: {},
  agents: {},
  agentStatus: {},
};

export function getState(): PageState {
  return state;
}

export function useStore<T>(select: (current: PageState) => T): T {
  return select(state);
}
