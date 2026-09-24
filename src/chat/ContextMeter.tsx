import { Tooltip } from "../components/Tooltip";
import type { AgentType } from "../state/types";
import type { ContextUsage } from "./types";

const tokens = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 });

/* An adapter names its own currency, and Intl throws on a code it does not know. */
function money({ amount, currency }: NonNullable<ContextUsage["cost"]>): string {
    try {
        return new Intl.NumberFormat("en", { style: "currency", currency, maximumFractionDigits: 2 }).format(amount);
    } catch {
        return `${amount.toFixed(2)} ${currency}`;
    }
}

function tone(percent: number): "steady" | "warm" | "hot" {
    if (percent >= 90) return "hot";
    if (percent >= 70) return "warm";
    return "steady";
}

export function ContextMeter({ usage, agent }: { usage: ContextUsage | null; agent: AgentType }) {
    if (!usage) return null;
    const percent = Math.min(100, Math.max(0, (usage.used / usage.size) * 100));
    const rounded = Math.round(percent);
    const details = (
        <span className="chat-context-tip">
            <strong>Context window</strong>
            <span>
                {rounded}% used · {tokens.format(usage.used)} of {tokens.format(usage.size)} tokens
            </span>
            {usage.cost && <span>Session cost {money(usage.cost)}</span>}
        </span>
    );
    return (
        <Tooltip label={details} side="top">
            <span
                className={`chat-context ${agent}`}
                data-tone={tone(percent)}
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
