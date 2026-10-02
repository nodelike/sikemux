import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";

type PeekEdge = "start" | "end";
type PeekPhase = "closed" | "open" | "closing";

interface RailPeekProps {
    edge: PeekEdge;
    children: ReactNode;
}

const CLOSE_DURATION_MS = 180;
const EDGE_REACH_PX = 28;

function withinEdgeReach(root: HTMLElement, edge: PeekEdge, event: PointerEvent) {
    const rect = root.getBoundingClientRect();
    if (event.clientY < rect.top || event.clientY > rect.bottom) return false;
    return edge === "start" ? event.clientX <= rect.left + EDGE_REACH_PX : event.clientX >= rect.right - EDGE_REACH_PX;
}

export function RailPeek({ edge, children }: RailPeekProps) {
    const [phase, setPhase] = useState<PeekPhase>("closed");
    const rootRef = useRef<HTMLDivElement>(null);
    const closeTimer = useRef<number | null>(null);
    const phaseRef = useRef<PeekPhase>("closed");
    phaseRef.current = phase;

    const clearCloseTimer = useCallback(() => {
        if (closeTimer.current === null) return;
        window.clearTimeout(closeTimer.current);
        closeTimer.current = null;
    }, []);

    const open = useCallback(() => {
        clearCloseTimer();
        setPhase("open");
    }, [clearCloseTimer]);

    const close = useCallback(
        (ignoreFocus = false) => {
            if (!ignoreFocus && rootRef.current?.contains(document.activeElement)) return;
            clearCloseTimer();
            setPhase("closing");
            closeTimer.current = window.setTimeout(() => {
                closeTimer.current = null;
                setPhase("closed");
            }, CLOSE_DURATION_MS);
        },
        [clearCloseTimer],
    );

    useEffect(() => {
        const onPointerMove = (event: PointerEvent) => {
            const root = rootRef.current;
            if (!root || event.buttons !== 0) return;
            const overPeek = event.target instanceof Node && root.contains(event.target);
            if (overPeek || withinEdgeReach(root, edge, event)) {
                if (phaseRef.current !== "open") open();
            } else if (phaseRef.current === "open") {
                close();
            }
        };
        window.addEventListener("pointermove", onPointerMove);
        return () => {
            window.removeEventListener("pointermove", onPointerMove);
            clearCloseTimer();
        };
    }, [edge, open, close, clearCloseTimer]);

    return (
        <div
            ref={rootRef}
            className={`rail-peek rail-peek--${edge}`}
            data-testid={`rail-peek-${edge}`}
            data-overlay
            onPointerEnter={open}
            onPointerLeave={() => close()}
            onFocusCapture={open}
            onBlurCapture={(event) => {
                if (!event.currentTarget.contains(event.relatedTarget)) close(true);
            }}>
            <div className="rail-peek-sensor" aria-hidden="true" />
            {phase !== "closed" && (
                <div
                    className={`rail-peek-panel rail-peek-panel--${phase}`}
                    aria-hidden={phase === "closing"}
                    onAnimationEnd={() => {
                        if (phase === "closing") {
                            clearCloseTimer();
                            setPhase("closed");
                        }
                    }}>
                    {children}
                </div>
            )}
        </div>
    );
}
