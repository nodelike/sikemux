import { useEffect, useRef, useState, type ReactNode } from "react";

type PeekEdge = "start" | "end";
type PeekPhase = "closed" | "open" | "closing";

interface RailPeekProps {
    edge: PeekEdge;
    children: ReactNode;
}

const CLOSE_DURATION_MS = 180;

export function RailPeek({ edge, children }: RailPeekProps) {
    const [phase, setPhase] = useState<PeekPhase>("closed");
    const rootRef = useRef<HTMLDivElement>(null);
    const closeTimer = useRef<number | null>(null);

    const clearCloseTimer = () => {
        if (closeTimer.current === null) return;
        window.clearTimeout(closeTimer.current);
        closeTimer.current = null;
    };

    const open = () => {
        clearCloseTimer();
        setPhase("open");
    };

    const close = (ignoreFocus = false) => {
        if (!ignoreFocus && rootRef.current?.contains(document.activeElement)) return;
        clearCloseTimer();
        setPhase("closing");
        closeTimer.current = window.setTimeout(() => {
            closeTimer.current = null;
            setPhase("closed");
        }, CLOSE_DURATION_MS);
    };

    useEffect(
        () => () => {
            clearCloseTimer();
        },
        [],
    );

    return (
        <div
            ref={rootRef}
            className={`rail-peek rail-peek--${edge}`}
            data-testid={`rail-peek-${edge}`}
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
