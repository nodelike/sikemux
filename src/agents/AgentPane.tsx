import { lazy, Suspense, useEffect } from "react";
import type { Session } from "../state/types";
import * as cmd from "../state/commands";
import { profileOf } from "../state/commands/agentLaunchCommand";
import { useStore } from "../state/store";

const AgentSurface = lazy(() => import("../chat/AgentSurface").then((module) => ({ default: module.AgentSurface })));

/** An agent's pane. The pane's id is the agent's id, so the record is looked up by it. A sleeping agent wakes once its pane is on screen. */
export function AgentPane({ paneId, session, visible }: { paneId: string; session: Session; visible: boolean }) {
    const agent = useStore((state) => state.agents[paneId]);
    const profile = useStore((state) => (agent ? profileOf(state, agent) : undefined));
    const asleep = agent?.launchState === "dormant";
    useEffect(() => {
        if (asleep && visible) cmd.resumeAgent(paneId);
    }, [asleep, visible, paneId]);
    if (!agent || asleep) return null;
    return (
        <Suspense fallback={null}>
            <AgentSurface agent={agent} session={session} profile={profile} visible={visible} />
        </Suspense>
    );
}
