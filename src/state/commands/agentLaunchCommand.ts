import type { StoreState } from "../store";
import type { Agent, ProviderProfile } from "../types";

/** The account an agent runs under, when it is one of the person's profiles for that provider. */
export function profileOf(state: Pick<StoreState, "providerProfiles">, agent: Pick<Agent, "profileId" | "type">): ProviderProfile | undefined {
    return agent.profileId ? state.providerProfiles.find((profile) => profile.id === agent.profileId && profile.provider === agent.type) : undefined;
}

/** The CLI an agent launches: its profile's, else the one it was started with. */
export function agentExecutablePath(
    state: Pick<StoreState, "providerProfiles">,
    agent: Pick<Agent, "profileId" | "type" | "executablePath">,
): string | undefined {
    return profileOf(state, agent)?.executablePath || agent.executablePath;
}
