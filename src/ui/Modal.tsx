import { useEffect, useRef, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useModalFocus } from "../hooks/useModalFocus";
import { useOccludeNativeViews } from "../state/nativeViews";
import { Kbd } from "./Kbd";

/** The app's sheet over a scrim: a titled card, its content, and a foot of buttons. Escape or a click outside closes it. */
export function Modal({
    title,
    glyph,
    danger = false,
    onClose,
    foot,
    children,
}: {
    title: string;
    glyph: ReactNode;
    danger?: boolean;
    onClose: () => void;
    foot: ReactNode;
    children?: ReactNode;
}) {
    const ref = useRef<HTMLDivElement>(null);
    useModalFocus(ref);
    useOccludeNativeViews(true);

    // Capture Escape before the panes behind the scrim can act on it.
    useEffect(() => {
        const onKey = (event: KeyboardEvent) => {
            if (event.key !== "Escape" || (event.target instanceof Element && event.target.closest(".dd-menu"))) return;
            event.preventDefault();
            event.stopPropagation();
            onClose();
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, [onClose]);

    return createPortal(
        <div className="dlg-scrim" onMouseDown={onClose}>
            <div
                ref={ref}
                tabIndex={-1}
                className={`dlg${danger ? " danger" : ""}`}
                role="dialog"
                aria-modal="true"
                aria-label={title}
                onMouseDown={(event) => event.stopPropagation()}>
                <div className="dlg-head">
                    <span className="dlg-glyph" aria-hidden="true">
                        {glyph}
                    </span>
                    <h2 className="dlg-title">{title}</h2>
                </div>
                {children}
                <div className="dlg-foot">
                    <span className="dlg-hint">
                        <Kbd>esc</Kbd> cancel
                    </span>
                    {foot}
                </div>
            </div>
        </div>,
        document.body,
    );
}
