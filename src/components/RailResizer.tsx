import { useLayoutEffect, type KeyboardEvent, type PointerEvent as ReactPointerEvent } from "react";
import * as cmd from "../state/commands";
import { getState, useStore } from "../state/store";
import { RAIL_WIDTH, type RailEdge } from "../lib/railWidths";

const widthOf = (edge: RailEdge) => (edge === "start" ? getState().sideRailWidth : getState().agentRailWidth);

export function useRailWidthVars(): void {
    const side = useStore((s) => s.sideRailWidth);
    const agent = useStore((s) => s.agentRailWidth);
    useLayoutEffect(() => {
        const root = document.documentElement.style;
        root.setProperty("--side-rail-w", `${side}px`);
        root.setProperty("--agent-rail-w", `${agent}px`);
    }, [side, agent]);
}

export function RailResizer({ edge }: { edge: RailEdge }) {
    const width = useStore((s) => (edge === "start" ? s.sideRailWidth : s.agentRailWidth));
    const outward = edge === "start" ? 1 : -1;

    const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
        e.preventDefault();
        const handle = e.currentTarget;
        handle.setPointerCapture(e.pointerId);
        const start = e.clientX;
        const startWidth = widthOf(edge);

        let frame: number | null = null;
        let pending: number | null = null;
        const commitPending = () => {
            frame = null;
            if (pending === null) return;
            cmd.setRailWidth(edge, pending);
            pending = null;
        };
        const move = (ev: PointerEvent) => {
            pending = startWidth + outward * (ev.clientX - start);
            if (frame == null) frame = window.requestAnimationFrame(commitPending);
        };
        const up = () => {
            if (frame != null) window.cancelAnimationFrame(frame);
            commitPending();
            handle.removeEventListener("pointermove", move);
            handle.removeEventListener("pointerup", up);
            handle.removeEventListener("pointercancel", up);
        };
        handle.addEventListener("pointermove", move);
        handle.addEventListener("pointerup", up);
        handle.addEventListener("pointercancel", up);
    };

    const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
        const direction = e.key === "ArrowLeft" ? -1 : e.key === "ArrowRight" ? 1 : 0;
        if (!direction) return;
        e.preventDefault();
        const step = e.shiftKey ? 40 : 16;
        cmd.setRailWidth(edge, widthOf(edge) + outward * direction * step);
    };

    return (
        <div
            className={`divider divider-row rail-resizer rail-resizer--${edge}`}
            role="separator"
            tabIndex={0}
            aria-orientation="vertical"
            aria-label={edge === "start" ? "Resize the project rail" : "Resize the agent rail"}
            aria-valuemin={RAIL_WIDTH[edge].min}
            aria-valuemax={RAIL_WIDTH[edge].max}
            aria-valuenow={width}
            title="Drag or use arrow keys to resize"
            onPointerDown={onPointerDown}
            onKeyDown={onKeyDown}
        />
    );
}
