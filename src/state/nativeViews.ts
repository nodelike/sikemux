import { useEffect, useSyncExternalStore } from "react";
import { externalValue, useExternalValue } from "@shared/lib/externalValue";
import { mergeHoles, watchOverlays } from "./overlayHoles";

/* Native child views (the browser pages) paint above every DOM element, so an
   overlay that must show over them asks for them to step aside while it is
   open. Depth-counted: overlapping overlays compose and release in any order. */
let depth = 0;
const occluded = externalValue(false);

export const nativeViewsOccluded = occluded.get;

export function occludeNativeViews(): () => void {
    depth += 1;
    occluded.set(true);
    let released = false;
    return () => {
        if (released) return;
        released = true;
        depth -= 1;
        occluded.set(depth > 0);
    };
}

export function useNativeViewsOccluded(): boolean {
    return useExternalValue(occluded);
}

/** Hold the native views aside for as long as `active` stays true. */
export function useOccludeNativeViews(active: boolean): void {
    useEffect(() => {
        if (!active) return;
        return occludeNativeViews();
    }, [active]);
}

/* A toast or a menu is too small to send a page away for, so the page leaves
   a hole where it sits instead. The holes are found by watching the DOM for
   anything floating, for as long as a page is reading them. Rects are in the
   window's CSS pixels. */
export interface NativeViewHole {
    x: number;
    y: number;
    width: number;
    height: number;
    radius: number;
}

const NO_HOLES: NativeViewHole[] = [];
const holesByOwner = new Map<object, NativeViewHole[]>();
const OVERLAYS = {};
const holes = externalValue(NO_HOLES, () => watchOverlays((next) => setNativeViewHoles(OVERLAYS, next)));

/** Replace the holes `owner` asked for. An empty list withdraws them. */
export function setNativeViewHoles(owner: object, next: NativeViewHole[]): void {
    const previous = holesByOwner.get(owner) ?? NO_HOLES;
    if (next.length === previous.length && next.every((hole, i) => sameHole(hole, previous[i]))) return;
    if (next.length) holesByOwner.set(owner, next);
    else holesByOwner.delete(owner);
    holes.set(holesByOwner.size ? mergeHoles([...holesByOwner.values()].flat()) : NO_HOLES);
}

function sameHole(a: NativeViewHole, b: NativeViewHole): boolean {
    return a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height && a.radius === b.radius;
}

const watchNothing = () => () => {};

/** The holes to cut while `wanted`; the DOM is watched for them only while some page wants them. */
export function useNativeViewHoles(wanted = true): NativeViewHole[] {
    return useSyncExternalStore(wanted ? holes.subscribe : watchNothing, () => (wanted ? holes.get() : NO_HOLES));
}

/* A swipe slides the stage sideways by transform, which carries every pane on
   it somewhere else without a scroll or a resize to say so. A native view is
   placed by measuring the DOM, so while the stage moves it has to measure every
   frame, and one loop does that for all of them. */
const moving = externalValue(false);
let frame = 0;
const followers = new Set<() => void>();

function tick() {
    frame = requestAnimationFrame(tick);
    for (const follower of followers) follower();
}

export const stageMoving = moving.get;

function setStageMoving(next: boolean) {
    if (moving.get() === next) return;
    if (next) frame = requestAnimationFrame(tick);
    else {
        cancelAnimationFrame(frame);
        frame = 0;
    }
    moving.set(next);
}

export function useStageMoving(): boolean {
    return useExternalValue(moving);
}

/**
 * Run `still` once the stage is not travelling. It waits a frame first, because
 * a slide is only announced after the effects of the screen it brings in.
 */
export function whenStageStill(still: () => void): () => void {
    let stop = () => {};
    const frame = requestAnimationFrame(() => {
        if (!moving.get()) return still();
        stop = moving.subscribe(() => {
            if (moving.get()) return;
            stop();
            still();
        });
    });
    return () => {
        cancelAnimationFrame(frame);
        stop();
    };
}

/* A swipe and a desk opening can overlap, so each says it is moving on its own. */
let panning = false;
let holds = 0;

function refreshStageMoving() {
    setStageMoving(panning || holds > 0);
}

/** Say that the stage is travelling for as long as `active` stays true. */
export function useStageMotion(active: boolean): void {
    useEffect(() => {
        panning = active;
        refreshStageMoving();
        return () => {
            panning = false;
            refreshStageMoving();
        };
    }, [active]);
}

/* Screens nobody can see keep the width they had while the stage changes width,
   so they lay out once when it settles rather than on every frame of the move. */
function holdHiddenScreens(held: boolean) {
    const area = document.querySelector<HTMLElement>(".window-area");
    if (!area) return;
    if (held) area.style.setProperty("--held-screen-width", `${area.getBoundingClientRect().width}px`);
    else area.style.removeProperty("--held-screen-width");
    area.classList.toggle("holding-screens", held);
}

/** Say that something on the stage is moving until the returned release is called. */
export function holdStageMotion(): () => void {
    holds += 1;
    if (holds === 1) holdHiddenScreens(true);
    refreshStageMoving();
    let released = false;
    return () => {
        if (released) return;
        released = true;
        holds -= 1;
        if (holds === 0) holdHiddenScreens(false);
        refreshStageMoving();
    };
}

/** Run `follow` on every frame the stage is travelling. */
export function onStageFrame(follow: () => void): () => void {
    followers.add(follow);
    return () => {
        followers.delete(follow);
    };
}
