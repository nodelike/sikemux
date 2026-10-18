import { useCallback, useEffect, useRef, useState } from "react";

/**
 * A strip that scrolls sideways. `className` names the ends it is clipped at, so
 * `.side-scroll` can fade them, and a mouse wheel moves it sideways.
 */
export function useSideScroll<T extends HTMLElement>() {
    const ref = useRef<T>(null);
    const [clipped, setClipped] = useState({ start: false, end: false });

    const measure = useCallback(() => {
        const el = ref.current;
        if (!el) return;
        const start = el.scrollLeft > 1;
        const end = el.scrollLeft + el.clientWidth < el.scrollWidth - 1;
        setClipped((was) => (was.start === start && was.end === end ? was : { start, end }));
    }, []);

    useEffect(() => {
        const el = ref.current;
        if (!el) return;
        const observer = new ResizeObserver(measure);
        observer.observe(el);
        const onWheel = (event: WheelEvent) => {
            if (event.deltaX !== 0 || el.scrollWidth <= el.clientWidth) return;
            event.preventDefault();
            el.scrollLeft += event.deltaY;
        };
        el.addEventListener("wheel", onWheel, { passive: false });
        return () => {
            observer.disconnect();
            el.removeEventListener("wheel", onWheel);
        };
    }, [measure]);

    const className = `side-scroll${clipped.start ? " clipped-start" : ""}${clipped.end ? " clipped-end" : ""}`;
    return { ref, className, measure };
}
