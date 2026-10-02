import { useRef, type ReactNode } from "react";
import { IconChevron } from "../ui/Icons";
import { HISTORY_CLEARANCE, HISTORY_MIN, ResizeHandle } from "./ResizeHandle";

/**
 * A panel at the foot of the git pane's left column: its header folds it, and while open its top edge resizes it
 * against the list above. History, a pull request's conversation and its commits are all one of these.
 */
export function FoldPanel({
    label,
    count,
    summary,
    badge,
    open,
    height,
    onToggle,
    onResize,
    children,
}: {
    label: string;
    count?: number | null;
    /** Shown in the header while folded, such as the newest commit's subject. */
    summary?: string | null;
    /** Sits at the header's end, open or folded. */
    badge?: ReactNode;
    open: boolean;
    height: number | null;
    onToggle: () => void;
    onResize: (height: number | null) => void;
    children: ReactNode;
}) {
    const panelRef = useRef<HTMLDivElement>(null);
    return (
        <>
            {open && (
                <ResizeHandle
                    targetRef={panelRef}
                    axis="y"
                    grows={-1}
                    min={HISTORY_MIN}
                    max={() => (panelRef.current?.parentElement?.clientHeight ?? HISTORY_MIN + HISTORY_CLEARANCE) - HISTORY_CLEARANCE}
                    size={height}
                    label={`Resize ${label.toLowerCase()}`}
                    className="git-history-split"
                    onResize={onResize}
                />
            )}
            <div
                ref={panelRef}
                className={`git-history${open ? " open" : ""}`}
                style={open && height ? { flex: `0 0 ${height}px`, minHeight: HISTORY_MIN } : undefined}>
                <div className="git-history-head">
                    <button type="button" className="git-history-toggle" aria-expanded={open} onClick={onToggle}>
                        <span className="git-history-chev">
                            <IconChevron size={10} />
                        </span>
                        <span className="git-label">{label}</span>
                        {!!count && <span className="git-count">{count}</span>}
                        {!open && summary && <span className="git-history-latest">{summary}</span>}
                    </button>
                    {badge}
                </div>
                {open && children}
            </div>
        </>
    );
}
