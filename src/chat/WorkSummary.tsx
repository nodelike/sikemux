import type { AgentType } from "../state/types";
import { AgentIcon, IconChevron } from "../ui/Icons";
import { callsLabel, workedLabel } from "./toolView";

export function WorkSummary({
    agent,
    took,
    calls,
    thought,
    open,
    onToggle,
}: {
    agent: AgentType;
    took: number | null;
    calls: number;
    thought: boolean;
    open: boolean;
    onToggle: () => void;
}) {
    return (
        <div className="chat-work">
            <button type="button" className="chat-tools-sum" aria-expanded={open} onClick={onToggle}>
                <span className={`chat-work-mark agent-glyph ${agent}`} aria-hidden="true">
                    <AgentIcon type={agent} size={17} />
                </span>
                <span>{workedLabel(took, thought)}</span>
                {calls > 0 && (
                    <>
                        <span className="chat-work-calls chat-work-dot" aria-hidden="true">
                            ·
                        </span>
                        <span className="chat-work-calls">{callsLabel(calls)}</span>
                    </>
                )}
                <IconChevron size={10} className="chat-tools-chevron" />
            </button>
        </div>
    );
}
