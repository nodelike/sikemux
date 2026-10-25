import { Tooltip } from "../ui/Tooltip";
import type { AgentType } from "../state/types";
import { contextShare, contextTone, money, tokenCount } from "@shared/chat/contextUsage";
import type { ContextUsage } from "@shared/chat/types";

export function ContextMeter({ usage, agent }: { usage: ContextUsage | null; agent: AgentType }) {
    if (!usage) return null;
    const percent = contextShare(usage) * 100;
    const rounded = Math.round(percent);
    const details = (
        <span className="chat-context-tip">
            <strong>Context window</strong>
            <span>
                {rounded}% used · {tokenCount(usage.used)} of {tokenCount(usage.size)} tokens
            </span>
            {usage.cost && <span>Session cost {money(usage.cost)}</span>}
        </span>
    );
    return (
        <Tooltip label={details} side="top">
            <span
                className={`chat-context ${agent}`}
                data-tone={contextTone(usage)}
                tabIndex={0}
                role="img"
                aria-label={`Context window ${rounded}% used`}>
                <svg viewBox="0 0 18 18" aria-hidden="true">
                    <circle className="chat-context-track" cx="9" cy="9" r="7" pathLength={100} />
                    <circle className="chat-context-fill" cx="9" cy="9" r="7" pathLength={100} strokeDasharray={`${percent} 100`} />
                </svg>
            </span>
        </Tooltip>
    );
}
