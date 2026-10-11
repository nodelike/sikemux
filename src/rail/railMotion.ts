import { useLayoutEffect, useRef } from "react";
import { animate, leavingRef } from "../lib/motion";
import { holdStageMotion } from "../state/nativeViews";

/*
 * A rail opens and closes like a drawer. It keeps its own width the whole time,
 * so nothing inside it rewraps; a negative margin on its stage side hands the
 * stage the room it is giving up, frame by frame, and a clip hides the part of
 * the rail the stage has moved over. The stage resizes on every frame, the way
 * it does while a rail is dragged wider.
 */

export const RAIL_MOTION_MS = 400;
const EASE = "cubic-bezier(0.25, 1, 0.5, 1)";

const isStart = (rail: HTMLElement) => rail.classList.contains("side-rail");

/** How far the rail must tuck under the stage to give back all of its room, gap included. */
function fullTuck(rail: HTMLElement): number {
    const gap = rail.parentElement ? parseFloat(getComputedStyle(rail.parentElement).columnGap) || 0 : 0;
    return rail.getBoundingClientRect().width + gap;
}

/** How far the rail is tucked under the stage right now, part way through a move or not. */
function currentTuck(rail: HTMLElement): number {
    const style = getComputedStyle(rail);
    return -(parseFloat(isStart(rail) ? style.marginRight : style.marginLeft) || 0);
}

function tucked(rail: HTMLElement, by: number, opacity: number): Keyframe {
    return isStart(rail)
        ? { marginRight: `${-by}px`, clipPath: `inset(0 ${by}px 0 0)`, opacity }
        : { marginLeft: `${-by}px`, clipPath: `inset(0 0 0 ${by}px)`, opacity };
}

/* A move turned around part way runs for the share of the distance left.
   An opening rail drops its frames when it lands, so no clip is left cutting off what pokes out of it. */
function move(rail: HTMLElement, from: number, to: number, full: number, fromOpacity: number): Animation | null {
    for (const running of rail.getAnimations()) running.cancel();
    const share = full > 0 ? Math.abs(to - from) / full : 1;
    const run = animate(rail, [tucked(rail, from, fromOpacity), tucked(rail, to, to === 0 ? 1 : 0)], {
        duration: Math.max(120, RAIL_MOTION_MS * Math.min(1, share)),
        easing: EASE,
        fill: to === 0 ? "none" : "forwards",
    });
    if (!run) return null;
    const release = holdStageMotion();
    void run.finished.catch(() => {}).finally(release);
    return run;
}

const leavingFrom = new WeakMap<HTMLElement, { tuck: number; opacity: number }>();

/** On a rail docked in the shell: closing tucks it back under the stage while the stage takes its room. */
export const leavingRail = leavingRef<HTMLElement>(
    (rail) => {
        /* Whatever followed it, such as its resize handle, may have gone with it, and the rail must stay on its own side of the stage. */
        if (isStart(rail)) rail.parentElement?.prepend(rail);
        else rail.parentElement?.append(rail);
        const from = leavingFrom.get(rail) ?? { tuck: 0, opacity: 1 };
        const full = fullTuck(rail);
        return move(rail, from.tuck, full, full, from.opacity);
    },
    {
        onRemove: (rail) => {
            // Only the docked rail, and only one on screen: the hover peek's copy has its own way out.
            if (!rail.parentElement?.classList.contains("body") || isStowed(rail)) return false;
            leavingFrom.set(rail, { tuck: currentTuck(rail), opacity: Number(getComputedStyle(rail).opacity) });
        },
    },
);

/* A docked rail stays mounted while it is hidden, so showing it again only moves it rather than building it. */
const isStowed = (rail: HTMLElement) => rail.style.display === "none";

function stow(rail: HTMLElement): void {
    for (const running of rail.getAnimations()) running.cancel();
    rail.style.display = "none";
}

function tuckAway(rail: HTMLElement): void {
    const midway = rail.getAnimations().length > 0;
    const from = midway ? currentTuck(rail) : 0;
    const opacity = midway ? Number(getComputedStyle(rail).opacity) : 1;
    const full = fullTuck(rail);
    const run = move(rail, from, full, full, opacity);
    if (!run) return stow(rail);
    void run.finished.then(
        () => stow(rail),
        () => {},
    );
}

function bringOut(rail: HTMLElement, selector: string): void {
    const midway = !isStowed(rail) && rail.getAnimations().length > 0;
    let from = midway ? currentTuck(rail) : Infinity;
    let opacity = midway ? Number(getComputedStyle(rail).opacity) : 0;
    rail.style.removeProperty("display");
    const full = fullTuck(rail);
    /* A rail that came back while its last copy was still closing takes over from where that one has got to. */
    const leaving = document.querySelector<HTMLElement>(`.shell > .body > ${selector}.is-leaving`);
    if (leaving) {
        from = currentTuck(leaving);
        opacity = Number(getComputedStyle(leaving).opacity);
        leaving.remove();
    }
    move(rail, Math.min(from, full), 0, full, opacity);
}

/** Opens and closes a docked rail from under the stage, but not when the window first draws it. */
export function useRailDock(visible: boolean, present: boolean, selector: string, drawn = true): void {
    const was = useRef<boolean | null>(null);
    useLayoutEffect(() => {
        if (!drawn) return;
        const open = visible && present;
        const first = was.current === null;
        const opened = !first && open && !was.current;
        const closed = !first && !open && !!was.current;
        was.current = open;
        const rail = document.querySelector<HTMLElement>(`.shell > .body > ${selector}:not(.is-leaving)`);
        if (!rail) return;
        if (opened) bringOut(rail, selector);
        else if (closed) tuckAway(rail);
        else if (!open) stow(rail);
    }, [visible, present, selector, drawn]);
}
