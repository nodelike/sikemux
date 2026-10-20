import type { StoreState } from "../state/store";
import type { AgentPermissionMode, AgentType, ProviderProfile } from "../state/types";
import { selectedProviderProfile } from "./agentProfiles";
import { isAgentType } from "./catalog";

type Defaults = Pick<
    StoreState,
    "projectSettings" | "providerProfiles" | "selectedProviderProfileIds" | "defaultAgentPermissionMode" | "lastAgentType"
>;

/** The agent New chat starts in the project at `cwd`: the project's pick, else the one launched last. */
export function projectAgentType(state: Defaults, cwd: string): AgentType | null {
    const picked = state.projectSettings[cwd]?.agent;
    return isAgentType(picked) ? picked : state.lastAgentType;
}

/** The mode a new chat in the project at `cwd` starts in: the project's pick, else the app's. */
export function projectPermissionMode(state: Defaults, cwd: string): AgentPermissionMode {
    const picked = state.projectSettings[cwd]?.permissionMode;
    return picked === "bypass" || picked === "workspace-write" ? picked : state.defaultAgentPermissionMode;
}

/** The account `type` signs in as in the project at `cwd`: the project's pick while it still exists, else the app's. */
export function projectProfile(state: Defaults, cwd: string, type: AgentType): ProviderProfile | undefined {
    const picked = state.projectSettings[cwd]?.accounts?.[type];
    return (
        state.providerProfiles.find((profile) => profile.id === picked && profile.provider === type) ??
        selectedProviderProfile(type, state.providerProfiles, state.selectedProviderProfileIds)
    );
}
