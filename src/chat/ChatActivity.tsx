import { useContext, useState } from "react";
import { useNow } from "../hooks/useNow";
import { AgentMark } from "../agents/AgentMark";
import type { Agent } from "../state/types";
import { ChatAgentContext } from "./chatAgent";
import { elapsedLabel } from "./durationLabel";
import { Shimmer } from "./Shimmer";

/* Keeps its own clock so a ticking second redraws this row alone, not the
   whole transcript. */
export function ChatActivity({ label, agentType }: { label: string; agentType: Agent["type"] }) {
    const agentId = useContext(ChatAgentContext).id;
    const [started] = useState(Date.now);
    const seconds = Math.max(0, Math.round((useNow() - started) / 1000));
    return (
        <div className="chat-activity" role="status">
            <span className={`chat-activity-mark agent-glyph ${agentType}`} aria-hidden="true">
                <AgentMark agentId={agentId} cli={agentType} size={21} />
            </span>
            <span className="chat-activity-label chat-shimmer">
                <Shimmer>{label}</Shimmer>
            </span>
            {seconds > 0 && (
                <span className="chat-activity-elapsed" aria-hidden="true">
                    {elapsedLabel(seconds)}
                </span>
            )}
        </div>
    );
}
