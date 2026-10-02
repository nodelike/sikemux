import { isPathWithin } from "../lib/paths";
import type { StoreState } from "../state/store";

export function worktreeHasLiveOwners(state: Pick<StoreState, "sessions" | "agents">, path: string, exceptAgentId?: string): boolean {
    const hasProject = Object.values(state.sessions).some((session) => session.kind === "project" && isPathWithin(session.cwd, path));
    if (hasProject) return true;
    return Object.values(state.agents).some((agent) => agent.id !== exceptAgentId && !!agent.cwd && isPathWithin(agent.cwd, path));
}
