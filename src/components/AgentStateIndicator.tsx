import { useLayoutEffect, useRef } from "react";
import { animate } from "../lib/motion";
import { AGENT_STATE_META } from "../state/agentStatus";
import type { AgentPresentationState } from "../state/types";
import { IconAgent, IconCommand } from "./Icons";

const BACKGROUND_LABEL = "Shells or monitors still running";
const TWINKLE_CELLS = [0, 1, 2, 3, 4, 5, 6, 7, 8];

/**
 * A twinkling grid while an agent works, a dot once it has something waiting for you,
 * and nothing while it sits idle — a rail of idle agents would be a column of
 * dots carrying no information, since the row already says the agent exists.
 * Left-over shells get the terminal glyph instead of a dot, so they do not read
 * as the same amber dot that means the agent is waiting on you.
 */
export function showsAgentState(state: AgentPresentationState, background = false): boolean {
    return state === "working" || state === "blocked" || state === "done" || background;
}

/**
 * A change of state is said once: the new mark pops in, and a mark that now
 * needs you sends two rings out and goes still. A mark that mounts in a state
 * just shows it, so a list drawing itself again does not replay anything.
 */
function useStateChange(tone: string) {
    const mark = useRef<HTMLSpanElement>(null);
    const last = useRef<string | null>(null);
    useLayoutEffect(() => {
        const previous = last.current;
        last.current = tone;
        const el = mark.current;
        if (previous === null || previous === tone || !el) return;
        animate(el, [{ opacity: 0, transform: "scale(0.4)" }, { opacity: 1, transform: "scale(1.18)", offset: 0.6 }, { transform: "scale(1)" }], {
            duration: 200,
        });
        if (tone !== "blocked") return;
        for (const delay of [0, 380]) {
            const ring = document.createElement("span");
            ring.className = "agent-state-ping";
            ring.setAttribute("aria-hidden", "true");
            el.append(ring);
            const run = animate(
                ring,
                [
                    { transform: "scale(0.6)", opacity: 0.9 },
                    { transform: "scale(2.4)", opacity: 0 },
                ],
                {
                    duration: 520,
                    delay,
                    easing: "cubic-bezier(0.2, 0.6, 0.3, 1)",
                    fill: "backwards",
                },
            );
            if (run)
                run.finished.then(
                    () => ring.remove(),
                    () => ring.remove(),
                );
            else ring.remove();
        }
    }, [tone]);
    return mark;
}

export function AgentStateIndicator({
    state,
    unread = false,
    background = false,
}: {
    state: AgentPresentationState;
    unread?: boolean;
    background?: boolean;
}) {
    const shown = showsAgentState(state, background);
    const tone = state === "working" ? "working" : !shown ? "idle" : state === "blocked" ? "blocked" : state === "done" ? "done" : "background";
    const mark = useStateChange(tone);
    if (tone === "working") {
        const label = AGENT_STATE_META.working.label;
        return (
            <span ref={mark} className={`agent-activity state-working${unread ? " unread" : ""}`} title={label} aria-label={label} role="img">
                <span className="agent-state-loader" aria-hidden="true">
                    {TWINKLE_CELLS.map((cell) => (
                        <i key={cell} />
                    ))}
                </span>
            </span>
        );
    }
    if (tone === "idle") return null;
    const label = tone === "background" ? BACKGROUND_LABEL : AGENT_STATE_META[state].label;
    return (
        <span ref={mark} className={`agent-activity state-${tone}${unread ? " unread" : ""}`} title={label} aria-label={label} role="img">
            {tone === "background" ? <IconCommand size={11} className="agent-state-icon" /> : <span className="agent-state-dot" aria-hidden="true" />}
        </span>
    );
}

export function SubagentCount({ count }: { count: number }) {
    const label = `${count} ${count === 1 ? "subagent" : "subagents"} running`;
    return (
        <span className="subagent-count" title={label} aria-label={label} role="img">
            <IconAgent size={10} />
            <span aria-hidden="true">{count}</span>
        </span>
    );
}
