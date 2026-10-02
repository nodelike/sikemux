import type { AgentType, ProviderProfile, ProviderProfileSelection } from "../state/types";

export interface AgentRuntimeProfile {
    type: AgentType;
    profileId?: string;
    executablePath?: string;
    configPath?: string;
}

export function selectedProviderProfile(
    type: AgentType,
    profiles: readonly ProviderProfile[],
    selections: ProviderProfileSelection,
): ProviderProfile | undefined {
    const selected = selections[type];
    return selected ? profiles.find((profile) => profile.id === selected && profile.provider === type) : undefined;
}

export function selectedAgentRuntimeProfiles(profiles: readonly ProviderProfile[], selections: ProviderProfileSelection): AgentRuntimeProfile[] {
    return (["claude", "codex"] as const).map((type) => {
        const profile = selectedProviderProfile(type, profiles, selections);
        return {
            type,
            profileId: profile?.id,
            executablePath: profile?.executablePath,
            configPath: profile?.configPath,
        };
    });
}
