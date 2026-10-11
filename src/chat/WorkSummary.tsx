import { IconChevron } from "../ui/Icons";
import { durationLabel } from "./durationLabel";

export function WorkSummary({ took, calls, open, onToggle }: { took: number | null; calls: number; open: boolean; onToggle: () => void }) {
    return (
        <div className="chat-work">
            <button type="button" className="chat-tools-sum" aria-expanded={open} onClick={onToggle}>
                <span>{took === null ? "Worked" : `Worked for ${durationLabel(took)}`}</span>
                {calls > 0 && (
                    <>
                        <span className="chat-work-calls chat-work-dot" aria-hidden="true">
                            ·
                        </span>
                        <span className="chat-work-calls">{`${calls} tool ${calls === 1 ? "call" : "calls"}`}</span>
                    </>
                )}
                <IconChevron size={10} className="chat-tools-chevron" />
            </button>
        </div>
    );
}
