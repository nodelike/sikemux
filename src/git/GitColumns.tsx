import { useRef, type ReactNode } from "react";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { LEFT_MIN, RIGHT_MIN, ResizeHandle } from "./ResizeHandle";

/**
 * The git pane's two columns: a list or a conversation on the left, what it changed on the right. Changes, pull
 * requests and issues all sit in these, so the divider between them is one width for the whole pane.
 */
export function GitColumns({ paneId, left, right }: { paneId: string; left: ReactNode; right: ReactNode }) {
    const leftRef = useRef<HTMLDivElement>(null);
    const leftWidth = useStore((s) => s.gitViews[paneId]?.leftWidth ?? null);
    return (
        <div className="git-body">
            <div className="git-left" ref={leftRef} style={leftWidth ? { width: leftWidth } : undefined}>
                {left}
            </div>
            <ResizeHandle
                targetRef={leftRef}
                axis="x"
                grows={1}
                min={LEFT_MIN}
                max={() => (leftRef.current?.parentElement?.clientWidth ?? LEFT_MIN + RIGHT_MIN) - RIGHT_MIN}
                size={leftWidth}
                label="Resize the lists and the review"
                className="git-split"
                onResize={(width) => cmd.setGitView(paneId, { leftWidth: width })}
            />
            <div className="git-right">{right}</div>
        </div>
    );
}
