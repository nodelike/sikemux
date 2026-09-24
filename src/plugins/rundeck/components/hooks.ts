import { useEffect, useRef, useState, type RefObject } from "react";

export function useDebounced<T>(value: T, delayMs: number): T {
    const [settled, setSettled] = useState(value);
    useEffect(() => {
        const timer = window.setTimeout(() => setSettled(value), delayMs);
        return () => window.clearTimeout(timer);
    }, [value, delayMs]);
    return settled;
}

/** The current time, re-read every `intervalMs` while `ticking`. */
export function useNow(ticking: boolean, intervalMs = 1_000): number {
    const [now, setNow] = useState(() => Date.now());
    useEffect(() => {
        if (!ticking) return;
        setNow(Date.now());
        const timer = window.setInterval(() => setNow(Date.now()), intervalMs);
        return () => window.clearInterval(timer);
    }, [ticking, intervalMs]);
    return now;
}

/**
 * Keyboard for a small open menu: Escape closes it, arrow keys move between its
 * items, and the checked or first item takes focus when it opens.
 */
export function useMenuKeys(open: boolean, menuRef: RefObject<HTMLElement | null>, close: () => void, arrows = true): void {
    const closeRef = useRef(close);
    closeRef.current = close;
    useEffect(() => {
        if (!open) return;
        const items = () => [
            ...(menuRef.current?.querySelectorAll<HTMLElement>('[role="menuitem"]:not(:disabled), [role="option"]:not(:disabled)') ?? []),
        ];
        const first = items();
        (first.find((item) => item.getAttribute("aria-selected") === "true") ?? first[0])?.focus();
        const onKey = (event: KeyboardEvent) => {
            if (event.key === "Escape") {
                event.preventDefault();
                event.stopPropagation();
                closeRef.current();
                return;
            }
            if (!arrows) return;
            if (event.key !== "ArrowDown" && event.key !== "ArrowUp" && event.key !== "Home" && event.key !== "End") return;
            const list = items();
            if (!list.length) return;
            event.preventDefault();
            const at = list.indexOf(document.activeElement as HTMLElement);
            let next = 0;
            if (event.key === "ArrowDown") next = at < 0 ? 0 : (at + 1) % list.length;
            else if (event.key === "ArrowUp") next = at < 0 ? list.length - 1 : (at - 1 + list.length) % list.length;
            else if (event.key === "End") next = list.length - 1;
            list[next].focus();
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, [open, menuRef, arrows]);
}
