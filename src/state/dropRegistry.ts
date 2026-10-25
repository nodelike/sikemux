import { IS_WINDOWS } from "@shared/lib/platform";

export type DropPathsHandler = (paths: string[]) => void;
export type DropFolderHandler = (paths: string[]) => void;

/* Listed rather than weak-held: a drop the hit test misses is answered by
   looking through the surfaces that are on screen right now. Every surface
   unregisters when it unmounts. */
const pathHandlers = new Map<HTMLElement, DropPathsHandler>();
const folderHandlers = new WeakMap<HTMLElement, DropFolderHandler>();

let lastFocused: HTMLElement | null = null;
let watchingFocus = false;
let hovered: HTMLElement | null = null;

function inWindow(point: { x: number; y: number }): boolean {
    return point.x >= 0 && point.y >= 0 && point.x <= window.innerWidth && point.y <= window.innerHeight;
}

/* Where a native drag sits on the page. Windows reports the cursor in device
   pixels; macOS and Linux already report it in the units the page lays out in,
   though Tauri calls all three "physical". A point that lands outside the
   window came in the other units, so it is converted rather than trusted. */
export function nativeDropPoint(position: { x: number; y: number }): { x: number; y: number } {
    const ratio = window.devicePixelRatio || 1;
    const point = { x: position.x / (IS_WINDOWS ? ratio : 1), y: position.y / (IS_WINDOWS ? ratio : 1) };
    if (inWindow(point)) return point;
    const rescaled = IS_WINDOWS ? { ...position } : { x: position.x / ratio, y: position.y / ratio };
    return inWindow(rescaled) ? rescaled : point;
}

function watchFocus(): void {
    if (watchingFocus || typeof document === "undefined") return;
    watchingFocus = true;
    document.addEventListener("focusin", (event) => {
        const target = resolvePathDropTarget(event.target as HTMLElement | null);
        if (target) lastFocused = target;
    });
}

function onScreen(el: HTMLElement): boolean {
    if (!el.isConnected) return false;
    const rect = el.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return false;
    return el.ownerDocument.defaultView?.getComputedStyle(el).visibility !== "hidden";
}

export function registerPtyDrop(el: HTMLElement, fn: DropPathsHandler): () => void {
    return registerPathDrop(el, fn);
}

/** Register any UI surface that consumes native filesystem paths. */
export function registerPathDrop(el: HTMLElement, fn: DropPathsHandler): () => void {
    pathHandlers.set(el, fn);
    watchFocus();
    return () => {
        if (pathHandlers.get(el) !== fn) return;
        pathHandlers.delete(el);
        if (lastFocused === el) lastFocused = null;
        if (hovered === el) showPathDropHover(null);
    };
}

export function registerFolderDrop(el: HTMLElement, fn: DropFolderHandler): () => void {
    folderHandlers.set(el, fn);
    return () => {
        folderHandlers.delete(el);
    };
}

export function dispatchPaths(el: HTMLElement, paths: string[]): boolean {
    const fn = pathHandlers.get(el);
    if (!fn) return false;
    fn(paths);
    return true;
}

/** Find the nearest registered native-path consumer beneath a hit-tested node. */
export function resolvePathDropTarget(el: HTMLElement | null): HTMLElement | null {
    for (let target = el; target; target = target.parentElement) {
        if (pathHandlers.has(target)) return target;
    }
    return null;
}

/**
 * The smallest surface whose box holds the point. Catches a drop the hit test
 * answered with something else — a menu, a tooltip, a native layer the page
 * cannot see — while the cursor was still over a pane that takes paths.
 */
export function pathDropTargetAt(point: { x: number; y: number }): HTMLElement | null {
    let best: HTMLElement | null = null;
    let smallest = Infinity;
    for (const el of pathHandlers.keys()) {
        if (!onScreen(el)) continue;
        const rect = el.getBoundingClientRect();
        if (point.x < rect.left || point.x > rect.right || point.y < rect.top || point.y > rect.bottom) continue;
        const area = rect.width * rect.height;
        if (area >= smallest) continue;
        best = el;
        smallest = area;
    }
    return best;
}

/**
 * Where a drop lands when it misses every pane — on the tab bar, the rail, a
 * gap between panes. The session the user was last typing in takes it, so a
 * file dropped anywhere in the window still arrives somewhere.
 */
export function focusedPathDropTarget(): HTMLElement | null {
    const active = resolvePathDropTarget(document.activeElement as HTMLElement | null);
    if (active && onScreen(active)) return active;
    if (lastFocused && onScreen(lastFocused)) return lastFocused;
    const visible = [...pathHandlers.keys()].filter(onScreen);
    return visible.length === 1 ? visible[0] : null;
}

/** Light up the surface a drag is currently aimed at, and only that one. */
export function showPathDropHover(target: HTMLElement | null): void {
    if (hovered === target) return;
    if (hovered) delete hovered.dataset.nativePathDragOver;
    hovered = target;
    if (target) target.dataset.nativePathDragOver = "true";
}

/** Route a native drop from its deepest hit-tested node to the nearest owner. */
export function dispatchPathDrop(el: HTMLElement | null, paths: string[]): boolean {
    const target = resolvePathDropTarget(el);
    return target ? dispatchPaths(target, paths) : false;
}

export function dispatchFolder(el: HTMLElement, paths: string[]): boolean {
    const fn = folderHandlers.get(el);
    if (!fn) return false;
    fn(paths);
    return true;
}
