import { useLayoutEffect, useState, type RefObject } from "react";

/* One observer for every row that asks whether its text still fits, rather
   than one per row in a transcript that can hold hundreds of them. */
const fitChecks = new Map<Element, () => void>();
let fitObserver: ResizeObserver | null = null;

function watchFit(element: Element, check: () => void): () => void {
    fitObserver ??= new ResizeObserver((entries) => {
        for (const entry of entries) fitChecks.get(entry.target)?.();
    });
    fitChecks.set(element, check);
    fitObserver.observe(element);
    return () => {
        fitChecks.delete(element);
        fitObserver?.unobserve(element);
    };
}

export function useCutOff(ref: RefObject<HTMLElement | null>, watching: boolean): boolean {
    const [cutOff, setCutOff] = useState(false);
    useLayoutEffect(() => {
        const element = ref.current;
        if (!watching || !element) return;
        const check = () => setCutOff(element.scrollWidth > element.clientWidth + 1);
        check();
        return watchFit(element, check);
    }, [ref, watching]);
    return watching && cutOff;
}
