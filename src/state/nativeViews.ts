import { useEffect, useSyncExternalStore } from "react";

/* Native child views (the browser pages) paint above every DOM element, so an
   overlay that must show over them asks for them to step aside while it is
   open. Depth-counted: overlapping overlays compose and release in any order. */
let depth = 0;
const listeners = new Set<() => void>();

function notify() {
    for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
    listeners.add(listener);
    return () => {
        listeners.delete(listener);
    };
}

export function nativeViewsOccluded(): boolean {
    return depth > 0;
}

export function occludeNativeViews(): () => void {
    depth += 1;
    if (depth === 1) notify();
    let released = false;
    return () => {
        if (released) return;
        released = true;
        depth -= 1;
        if (depth === 0) notify();
    };
}

export function useNativeViewsOccluded(): boolean {
    return useSyncExternalStore(subscribe, nativeViewsOccluded, nativeViewsOccluded);
}

/** Hold the native views aside for as long as `active` stays true. */
export function useOccludeNativeViews(active: boolean): void {
    useEffect(() => {
        if (!active) return;
        return occludeNativeViews();
    }, [active]);
}

/* A toast is too small and too brief to send a page away for, so the page
   leaves a hole where it sits instead. Rects are in the window's CSS pixels. */
export interface NativeViewHole {
    x: number;
    y: number;
    width: number;
    height: number;
    radius: number;
}

const NO_HOLES: NativeViewHole[] = [];
let holes = NO_HOLES;
const holeListeners = new Set<() => void>();

function subscribeHoles(listener: () => void) {
    holeListeners.add(listener);
    return () => {
        holeListeners.delete(listener);
    };
}

function currentHoles(): NativeViewHole[] {
    return holes;
}

export function setNativeViewHoles(next: NativeViewHole[]): void {
    const same = next.length === holes.length && next.every((hole, i) => sameHole(hole, holes[i]));
    if (same) return;
    holes = next.length ? next : NO_HOLES;
    for (const listener of holeListeners) listener();
}

function sameHole(a: NativeViewHole, b: NativeViewHole): boolean {
    return a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height && a.radius === b.radius;
}

export function useNativeViewHoles(): NativeViewHole[] {
    return useSyncExternalStore(subscribeHoles, currentHoles, currentHoles);
}

/* A swipe slides the stage sideways by transform, which carries every pane on
   it somewhere else without a scroll or a resize to say so. A native view is
   placed by measuring the DOM, so while the stage moves it has to measure every
   frame, and one loop does that for all of them. */
let moving = false;
let frame = 0;
const watchers = new Set<() => void>();
const followers = new Set<() => void>();

function tick() {
    frame = requestAnimationFrame(tick);
    for (const follower of followers) follower();
}

function watch(watcher: () => void) {
    watchers.add(watcher);
    return () => {
        watchers.delete(watcher);
    };
}

export function stageMoving(): boolean {
    return moving;
}

function setStageMoving(next: boolean) {
    if (moving === next) return;
    moving = next;
    if (next) frame = requestAnimationFrame(tick);
    else {
        cancelAnimationFrame(frame);
        frame = 0;
    }
    for (const watcher of watchers) watcher();
}

export function useStageMoving(): boolean {
    return useSyncExternalStore(watch, stageMoving, stageMoving);
}

/**
 * Run `still` once the stage is not travelling. It waits a frame first, because
 * a slide is only announced after the effects of the screen it brings in.
 */
export function whenStageStill(still: () => void): () => void {
    let stop = () => {};
    const frame = requestAnimationFrame(() => {
        if (!moving) return still();
        stop = watch(() => {
            if (moving) return;
            stop();
            still();
        });
    });
    return () => {
        cancelAnimationFrame(frame);
        stop();
    };
}

/** Say that the stage is travelling for as long as `active` stays true. */
export function useStageMotion(active: boolean): void {
    useEffect(() => {
        setStageMoving(active);
        return () => setStageMoving(false);
    }, [active]);
}

/** Run `follow` on every frame the stage is travelling. */
export function onStageFrame(follow: () => void): () => void {
    followers.add(follow);
    return () => {
        followers.delete(follow);
    };
}
