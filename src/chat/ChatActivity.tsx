import { useEffect, useState } from "react";
import type { Agent } from "../state/types";
import { AgentIcon } from "../ui/Icons";
import { elapsedLabel } from "./durationLabel";

/* Keeps its own clock so a ticking second redraws this row alone, not the
   whole transcript. */
export function ChatActivity({ label, agentType }: { label: string; agentType: Agent["type"] }) {
    const [seconds, setSeconds] = useState(0);
    useEffect(() => {
        const started = Date.now();
        const timer = window.setInterval(() => setSeconds(Math.round((Date.now() - started) / 1000)), 1000);
        return () => window.clearInterval(timer);
    }, []);
    return (
        <div className="chat-activity" role="status">
            <span className={`chat-activity-mark agent-glyph ${agentType}`} aria-hidden="true">
                <AgentIcon type={agentType} size={21} />
            </span>
            <span className="chat-activity-label">{label}</span>
            {seconds > 0 && (
                <span className="chat-activity-elapsed" aria-hidden="true">
                    {elapsedLabel(seconds)}
                </span>
            )}
        </div>
    );
}
