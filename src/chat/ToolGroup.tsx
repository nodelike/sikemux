import { useLayoutEffect, useRef, useState, type CSSProperties, type RefObject } from "react";
import { animate, EASE_IN, foldedFrames, leavingRef } from "../lib/motion";
import { IconChevron } from "../ui/Icons";
import { toolDescription } from "./toolOutput";
import { LiveSeconds } from "./LiveSeconds";
import { ToolRow } from "./ToolRow";
import { durationLabel } from "./durationLabel";
import { toolKind, toolRunning } from "./toolLabels";
import type { ChatPart } from "./types";

/* A run of tool calls is worth watching while the agent is still adding to it
   and worth folding away once it has moved on, so it stays open until
   something else follows it or the turn ends, unless the reader says
   otherwise. Watching each call instead would shut the run in the gaps
   between calls, and open it again on the next one. */
/* Closing folds the calls away. The transcript measures every row as it
   changes size, so the rows below follow the fold rather than jumping. */
const foldToolBody = leavingRef<HTMLDivElement>((body) => {
    const [open, closed] = foldedFrames(body);
    body.style.overflow = "hidden";
    return animate(
        body,
        [
            { ...open, opacity: 1 },
            { ...closed, opacity: 0 },
        ],
        { duration: 140, easing: EASE_IN },
    );
});

/* Rows past these arrive with the group's own fade; stepping every row would
   keep the end of a long run hidden for seconds. */
const STEPPED_ROWS = 8;

/* Opening grows the calls in and steps them down one after another. Only a
   change of state animates: a group the list remounts on scroll just shows. */
function useToolGroupUnfold(group: RefObject<HTMLDivElement | null>, open: boolean): void {
    const was = useRef<boolean | null>(null);
    useLayoutEffect(() => {
        const before = was.current;
        was.current = open;
        const body = group.current?.querySelector<HTMLElement>(":scope > .chat-tools-body");
        if (before === null || before === open || !open || !body) return;
        const [rest, flat] = foldedFrames(body);
        body.style.overflow = "hidden";
        const run = animate(
            body,
            [
                { ...flat, opacity: 0 },
                { ...rest, opacity: 1 },
            ],
            { duration: 180 },
        );
        const settle = () => (body.style.overflow = "");
        if (run) run.finished.then(settle, settle);
        else settle();
        [...body.children].slice(0, STEPPED_ROWS).forEach((row, i) =>
            animate(
                row,
                [
                    { opacity: 0, transform: "translateX(-4px)" },
                    { opacity: 1, transform: "none" },
                ],
                { duration: 160, delay: 30 + i * 20, fill: "backwards" },
            ),
        );
    }, [group, open]);
}

export function ToolGroup({ tools, live }: { tools: Extract<ChatPart, { kind: "tool" }>[]; live: boolean }) {
    const [reader, setReader] = useState<boolean | null>(null);
    const current = tools.find((part) => toolRunning(part.tool));
    const open = reader ?? (live || current !== undefined);
    const groupRef = useRef<HTMLDivElement>(null);
    useToolGroupUnfold(groupRef, open);
    const spent = tools.reduce(
        (total, part) => total + (part.startedAt !== undefined && part.endedAt !== undefined ? part.endedAt - part.startedAt : 0),
        0,
    );
    /* One column for every call in the run, as wide as the longest name in it:
       a run of reads stays tight, one that called an MCP server gets the room. */
    const kindWidth = Math.min(16, Math.max(4, ...tools.map((part) => toolKind(part.tool).length)));
    // While a call runs, the header says what Claude said it is for; a finished run counts its calls.
    const said = current ? toolDescription(current.tool) : null;
    return (
        <div className="chat-tools" ref={groupRef}>
            <button type="button" className={`chat-tools-sum${current ? " live" : ""}`} aria-expanded={open} onClick={() => setReader(!open)}>
                {said ? (
                    <>
                        <span className="chat-tools-label said">{said}</span>
                        {tools.length > 1 && <span className="chat-tools-calls">{tools.length} calls</span>}
                    </>
                ) : (
                    <span className="chat-tools-label">
                        {tools.length} tool {tools.length === 1 ? "call" : "calls"}
                    </span>
                )}
                {current ? (
                    <LiveSeconds key={current.id} since={current.startedAt} spent={spent} />
                ) : (
                    spent > 0 && <span className="chat-tools-time">{durationLabel(spent)}</span>
                )}
                <IconChevron size={10} className="chat-tools-chevron" />
            </button>
            {open && (
                <div className="chat-tools-body" ref={foldToolBody} style={{ "--chat-kind": `${kindWidth}ch` } as CSSProperties}>
                    {tools.map((part) => (
                        <ToolRow key={part.id} part={part} />
                    ))}
                </div>
            )}
        </div>
    );
}
