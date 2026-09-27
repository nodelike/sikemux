import { useLayoutEffect, useRef } from "react";
import { animate, type Box, contentBox, EASE_LEAVE, leavingRef } from "../lib/motion";

/*
 * A rail opening or closing never animates its width: the stage beside it
 * would resize every frame, and every terminal in it would refit and resize
 * its program each time. The stage takes its new width once, and only the
 * rail moves: in from its edge when it opens, and out over the stage when it
 * closes.
 */

const SLIDE = 12;
const docked = new WeakMap<HTMLElement, Box>();

const edgeOf = (rail: HTMLElement) => (rail.classList.contains("side-rail") ? -1 : 1);

/** On a rail docked in the shell: closing lifts it out of the layout where it stood, and slides it away. */
export const leavingRail = leavingRef<HTMLElement>(
    (rail) => {
        const box = docked.get(rail);
        if (box) {
            Object.assign(rail.style, {
                position: "absolute",
                left: `${box.left}px`,
                top: `${box.top}px`,
                width: `${box.width}px`,
                height: `${box.height}px`,
                margin: "0",
                zIndex: "5",
            });
        }
        return animate(
            rail,
            [
                { opacity: 1, transform: "none" },
                { opacity: 0, transform: `translateX(${SLIDE * edgeOf(rail)}px)` },
            ],
            {
                duration: 120,
                easing: EASE_LEAVE,
            },
        );
    },
    {
        onRemove: (rail) => {
            // Only the docked rail: the hover peek's copy has its own way out.
            const shell = rail.parentElement;
            if (!shell?.classList.contains("body")) return false;
            docked.set(rail, contentBox(rail.getBoundingClientRect(), shell));
        },
    },
);

/** Slides a rail in from its edge when it opens, but not when the window first draws it. */
export function useRailEntrance(visible: boolean, selector: string): void {
    const was = useRef(visible);
    useLayoutEffect(() => {
        const opened = visible && !was.current;
        was.current = visible;
        if (!opened) return;
        const rail = document.querySelector<HTMLElement>(`.shell > .body > ${selector}:not(.is-leaving)`);
        if (!rail) return;
        animate(
            rail,
            [
                { opacity: 0, transform: `translateX(${SLIDE * edgeOf(rail)}px)` },
                { opacity: 1, transform: "none" },
            ],
            { duration: 180 },
        );
    }, [visible, selector]);
}
