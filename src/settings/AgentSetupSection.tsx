import { useMemo } from "react";
import { installAgent, signInAgent } from "../agents/AgentInstall";
import { selectedAgentRuntimeProfiles } from "../agents/agentProfiles";
import { agentStatusLabel } from "../agents/agentStatus";
import { agentAbout, installCommand, loginArgs } from "@shared/agents/catalog";
import type { AgentInfo } from "../api/agents";
import { useResource } from "../state/resources";
import { agentSetupR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { AgentIcon } from "../ui/Icons";
import { SettingsSection } from "./SettingsLayout";

export const AGENT_SETUP_SECTION = "Agents on this machine";

/** Every agent Sikemux can run, with what it needs next: nothing, an install, or a fix. */
export function AgentSetupSection() {
    const profiles = useStore((s) => s.providerProfiles);
    const selections = useStore((s) => s.selectedProviderProfileIds);
    const runtimeProfiles = useMemo(() => selectedAgentRuntimeProfiles(profiles, selections), [profiles, selections]);
    const catalog = useResource(agentSetupR, runtimeProfiles);
    const agents = catalog.data ?? [];
    const ready = agents.filter((agent) => agent.available !== false && agent.status?.state === "ready").length;
    return (
        <SettingsSection
            title={AGENT_SETUP_SECTION}
            meta={catalog.data ? `${ready} ready` : undefined}
            sub="Install an agent here and it lands where its own updater expects, so it works in your terminal too.">
            {!catalog.data ? (
                <p className="agent-setup-note">{catalog.status === "error" ? "Sikemux could not look for agents." : "Looking for agents…"}</p>
            ) : (
                <ul className="agent-setup-list">
                    {agents.map((agent) => (
                        <AgentSetupRow key={agent.profileId ?? agent.type} agent={agent} />
                    ))}
                </ul>
            )}
        </SettingsSection>
    );
}

function AgentSetupRow({ agent }: { agent: AgentInfo }) {
    const state = agent.status?.state;
    const installable = !!installCommand(agent.type);
    const detail = agent.status?.state === "broken" ? agent.status.reason : state === "missing" ? agentAbout(agent.type) : null;
    return (
        <li className={`agent-setup-row${agent.available === false ? " is-missing" : ""}`}>
            <span className={`agent-glyph ${agent.type}`}>
                <AgentIcon type={agent.type} size={18} />
            </span>
            <span className="agent-setup-name">
                <b>{agent.label}</b>
                <small className={state ? `is-${state}` : undefined}>{agentStatusLabel(agent.status) ?? "Found"}</small>
                {detail && <small className="agent-setup-detail">{detail}</small>}
            </span>
            {state === "signedOut" && loginArgs(agent.type) && (
                <button type="button" className="settings-btn" onClick={() => signInAgent(agent)}>
                    Sign in
                </button>
            )}
            {installable && (state === "missing" || state === "broken") && (
                <button type="button" className="settings-btn" onClick={() => installAgent(agent.type)}>
                    {state === "broken" ? "Reinstall" : "Install"}
                </button>
            )}
        </li>
    );
}
