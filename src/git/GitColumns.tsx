import { createContext, useContext, useLayoutEffect, useRef, type ReactNode } from "react";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { PaneField } from "../ui/ShaderField";
import { LEFT_MIN, RIGHT_MIN, ResizeHandle } from "./ResizeHandle";
import { preferencesOf } from "../state/preferences";

/** Lets columns inside a framed host screen take the frame onto their right column, so the left stands outside it. */
export const ShellFrameContext = createContext<{ claim: () => () => void } | null>(null);

/**
 * The git pane's two columns: a list or a conversation on the left, what it changed on the right. Changes, pull
 * requests and issues all sit in these, so the divider between them is one width for the whole pane.
 */
export function GitColumns({
    paneId,
    left,
    right,
    framed = false,
}: {
    paneId: string;
    left: ReactNode;
    right: ReactNode;
    /** The right column is the pane's surface, with its frame, ground and picture, and the left stands beside it. */
    framed?: boolean;
}) {
    const leftRef = useRef<HTMLDivElement>(null);
    const paneShader = useStore((s) => preferencesOf(s).paneShader);
    const shellFrame = useContext(ShellFrameContext);
    useLayoutEffect(() => shellFrame?.claim(), [shellFrame]);
    framed ||= !!shellFrame;
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
            <div className={`git-right${framed ? " git-framed" : ""}`}>
                {framed && <PaneField enabled={paneShader} />}
                {right}
            </div>
        </div>
    );
}
