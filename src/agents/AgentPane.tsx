import { lazy, Suspense } from "react";
import type { Session } from "../state/types";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";

const AgentSurface = lazy(() => import("../chat/AgentSurface").then((module) => ({ default: module.AgentSurface })));

/** An agent's pane. The pane's id is the agent's id, so the record is looked up by it. */
export function AgentPane({ paneId, session, visible }: { paneId: string; session: Session; visible: boolean }) {
    const agent = useStore((state) => state.agents[paneId]);
    const profile = useStore((state) =>
        agent?.profileId ? state.providerProfiles.find((candidate) => candidate.id === agent.profileId) : undefined,
    );
    if (!agent) return null;
    return (
        <>
            {agent.launchState === "dormant" ? (
                <div className="agent-dormant" role="group" aria-label={`${agent.title} is ready to resume`}>
                    <span className={`agent-dormant-notch ${agent.type}`} aria-hidden="true" />
                    <span className="agent-dormant-kicker">sleeping</span>
                    <strong>{agent.title}</strong>
                    <span>This resumable agent is using no live terminal process.</span>
                    <button type="button" onClick={() => cmd.resumeAgent(agent.id)}>
                        Resume {agent.type}
                    </button>
                </div>
            ) : (
                <Suspense fallback={<div className="agent-transport-switching">Opening agent session…</div>}>
                    <AgentSurface agent={agent} session={session} profile={profile} visible={visible} />
                </Suspense>
            )}
        </>
    );
}
