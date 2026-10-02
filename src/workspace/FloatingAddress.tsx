import { useLayoutEffect, useMemo, useRef, useState, type RefObject } from "react";
import { createPortal } from "react-dom";
import { AddressBar } from "./AddressBar";

interface Place {
    left: number;
    top: number;
    width: number;
}

const MAX_WIDTH = 680;
const MARGIN = 16;

/**
 * The address opened from the keyboard: a field and its suggestions floating
 * over the middle of the page, rather than hanging from the toolbar.
 */
export function FloatingAddress({
    over,
    tabId,
    pageAddress,
    onGo,
    onClose,
}: {
    /** The page area the panel sits over. */
    over: RefObject<HTMLElement | null>;
    tabId: string | undefined;
    pageAddress: string;
    onGo: (url: string) => void;
    onClose: () => void;
}) {
    const panelRef = useRef<HTMLDivElement>(null);
    const [area, setArea] = useState<DOMRect | null>(null);
    const [height, setHeight] = useState(0);

    useLayoutEffect(() => {
        const page = over.current;
        if (!page) return;
        const measure = () => setArea(page.getBoundingClientRect());
        measure();
        const observer = new ResizeObserver(measure);
        observer.observe(page);
        window.addEventListener("resize", measure);
        return () => {
            observer.disconnect();
            window.removeEventListener("resize", measure);
        };
    }, [over]);

    useLayoutEffect(() => {
        const panel = panelRef.current;
        if (!panel) return;
        const measure = () => setHeight(panel.offsetHeight);
        measure();
        const observer = new ResizeObserver(measure);
        observer.observe(panel);
        return () => observer.disconnect();
    }, []);

    /* The field sits a little above the middle of the page and the list grows
       down from it, rising only as far as a long list needs to stay on the page. */
    const place = useMemo<Place | null>(() => {
        if (!area) return null;
        const width = Math.min(MAX_WIDTH, area.width - 2 * MARGIN);
        return {
            left: area.left + (area.width - width) / 2,
            top: Math.max(area.top + MARGIN, Math.min(area.top + area.height * 0.36, area.bottom - height - MARGIN)),
            width,
        };
    }, [area, height]);

    return createPortal(
        <div
            ref={panelRef}
            className="address-float"
            role="dialog"
            aria-label="Open address"
            style={place ? { left: place.left, top: place.top, width: place.width } : { visibility: "hidden" }}>
            <AddressBar floating tabId={tabId} pageAddress={pageAddress} onGo={onGo} onLeave={onClose} />
        </div>,
        document.body,
    );
}
