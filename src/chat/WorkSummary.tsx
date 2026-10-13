import { IconChevron } from "../ui/Icons";
import { callsLabel, workedLabel } from "./toolView";

export function WorkSummary({
    took,
    calls,
    thought,
    open,
    onToggle,
}: {
    took: number | null;
    calls: number;
    thought: boolean;
    open: boolean;
    onToggle: () => void;
}) {
    return (
        <div className="chat-work">
            <button type="button" className="chat-tools-sum" aria-expanded={open} onClick={onToggle}>
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
