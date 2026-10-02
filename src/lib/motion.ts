/** Whether the system has been asked for less animation. */
export function prefersReducedMotion(): boolean {
    return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

export const EASE_OUT = "cubic-bezier(0.16, 1, 0.3, 1)";
export const EASE_IN = "cubic-bezier(0.4, 0, 1, 1)";
/** For two things trading places at once, such as one tree folding while another opens. */
export const EASE_SWAP = "cubic-bezier(0.65, 0, 0.35, 1)";
/** For something leaving on a click: most of the movement happens straight away. */
export const EASE_LEAVE = "cubic-bezier(0.25, 0.8, 0.25, 1)";

/** Whether this element may animate now. Also false under jsdom, which has no Web Animations. */
export function canAnimate(el: Element | null | undefined): el is HTMLElement {
    return !!el && typeof (el as HTMLElement).animate === "function" && !prefersReducedMotion();
}

export function animate(el: Element | null | undefined, frames: Keyframe[], options: KeyframeAnimationOptions): Animation | null {
    if (!canAnimate(el)) return null;
    return el.animate(frames, { easing: EASE_OUT, ...options });
}

export interface Box {
    left: number;
    top: number;
    width: number;
    height: number;
}

/** A box in the container's scrolled content, so it stays right while the container scrolls. */
export function contentBox(rect: DOMRect | Box, container: HTMLElement): Box {
    const edge = container.getBoundingClientRect();
    return {
        left: rect.left - edge.left - container.clientLeft + container.scrollLeft,
        top: rect.top - edge.top - container.clientTop + container.scrollTop,
        width: rect.width,
        height: rect.height,
    };
}

const px = (box: Box) => ({ left: `${box.left}px`, top: `${box.top}px`, width: `${box.width}px`, height: `${box.height}px` });

/**
 * Hands state classes over without their own colour transitions. Rows and tabs
 * fade their tint over 90ms, so a selection moving by animation would otherwise
 * blink out at one end and fade in again at the other.
 */
export function withoutTransitions(elements: HTMLElement[], change: () => void): void {
    const kept = elements.map((el) => el.style.transition);
    for (const el of elements) el.style.transition = "none";
    change();
    requestAnimationFrame(() =>
        requestAnimationFrame(() => {
            elements.forEach((el, i) => (el.style.transition = kept[i]));
        }),
    );
}

/**
 * Slides a copy of the selected item's surface from where the selection was to
 * where it is now. The copy lives inside `container`, so it scrolls with the
 * items and is clipped by the container's edge, and it lands on `to` in the
 * same frame the real surface comes back.
 */
export function glideSelection(container: HTMLElement, from: Box, to: HTMLElement, previous?: HTMLElement | null, duration = 180): void {
    if (!canAnimate(to)) return;
    // Before anything reads style: otherwise the old tint starts fading out under the copy.
    if (previous) withoutTransitions([previous], () => undefined);
    const kept = to.style.transition;
    to.style.transition = "none";
    const target = contentBox(to.getBoundingClientRect(), container);
    if (Math.abs(target.left - from.left) < 1 && Math.abs(target.top - from.top) < 1 && Math.abs(target.width - from.width) < 1) {
        to.style.transition = kept;
        return;
    }
    const surface = getComputedStyle(to);
    // A tint drawn as a background image sits over a ground every item shares, so only the tint moves.
    const tintLayer = surface.backgroundImage !== "none";
    const ghost = document.createElement("div");
    ghost.className = "selection-glide";
    ghost.setAttribute("aria-hidden", "true");
    Object.assign(ghost.style, {
        ...px(target),
        background: tintLayer ? surface.backgroundImage : surface.backgroundColor,
        borderRadius: surface.borderTopLeftRadius,
        boxShadow: surface.boxShadow,
        border: `${surface.borderTopWidth} ${surface.borderTopStyle} ${surface.borderTopColor}`,
    });
    to.style.setProperty(tintLayer ? "background-image" : "background", tintLayer ? "none" : "transparent", "important");
    to.style.setProperty("border-color", "transparent", "important");
    to.style.setProperty("box-shadow", "none", "important");
    if (getComputedStyle(container).position === "static") container.style.position = "relative";
    container.append(ghost);
    const run = ghost.animate([px(from), px(target)], { duration, easing: EASE_OUT, fill: "forwards" });
    const land = () => {
        for (const property of ["background", "background-image", "border-color", "box-shadow"]) to.style.removeProperty(property);
        ghost.remove();
        requestAnimationFrame(() => requestAnimationFrame(() => (to.style.transition = kept)));
    };
    run.finished.then(land, land);
}

/** Everything that gives a box its height, so folding it really reaches zero. */
export function foldedFrames(el: HTMLElement): [Keyframe, Keyframe] {
    const style = getComputedStyle(el);
    return [
        { height: `${el.offsetHeight}px`, paddingTop: style.paddingTop, paddingBottom: style.paddingBottom },
        { height: "0px", paddingTop: "0px", paddingBottom: "0px" },
    ];
}

/**
 * A ref that keeps its element on screen after React removes it, long enough
 * for `leave` to play. The element goes back where it was, inert and hidden
 * from assistive tech, and is dropped when the animation ends. It has to be one
 * stable function (module-level or memoised): React also runs a ref's cleanup
 * when the ref function changes.
 */
export function leavingRef<T extends HTMLElement>(
    leave: (el: T) => Animation | null | undefined,
    hooks: { onMount?: (el: T) => void; onRemove?: (el: T) => boolean | void } = {},
): (el: T | null) => (() => void) | undefined {
    return (el) => {
        if (!el) return undefined;
        hooks.onMount?.(el);
        return () => {
            const parent = el.parentNode;
            const next = el.nextSibling;
            // onRemove sees the element still in place, and can veto the exit by returning false.
            if (hooks.onRemove?.(el) === false) return;
            if (!parent || !el.isConnected || !canAnimate(el)) return;
            queueMicrotask(() => {
                if (!(parent as Node).isConnected || el.isConnected) return;
                el.inert = true;
                el.setAttribute("aria-hidden", "true");
                el.classList.add("is-leaving");
                for (const node of [el, ...el.querySelectorAll("[id]")]) node.removeAttribute("id");
                parent.insertBefore(el, next && next.parentNode === parent ? next : null);
                const run = leave(el);
                if (!run) return el.remove();
                run.finished.then(
                    () => el.remove(),
                    () => el.remove(),
                );
            });
        };
    };
}

/**
 * For a backdrop and the panel it holds, such as a palette: they appear on the
 * first frame, and close by fading while the panel shrinks a touch. The panel
 * keeps whatever transform already places it.
 */
export const leavingOverlay = leavingRef<HTMLElement>((backdrop) => {
    const panel = backdrop.firstElementChild as HTMLElement | null;
    if (panel) {
        const placed = getComputedStyle(panel).transform;
        const base = placed === "none" ? "" : `${placed} `;
        animate(panel, [{ transform: `${base}scale(1)` }, { transform: `${base}scale(0.985)` }], {
            duration: 100,
            easing: EASE_IN,
            fill: "forwards",
        });
    }
    return animate(backdrop, [{ opacity: 1 }, { opacity: 0 }], { duration: 100, easing: EASE_IN, fill: "forwards" });
});

/** A menu opens on the first frame and closes with a short fade. */
export const leavingMenu = leavingRef<HTMLElement>((menu) =>
    animate(menu, [{ opacity: 1 }, { opacity: 0 }], { duration: 80, easing: EASE_IN, fill: "forwards" }),
);

/** Shares one element between a component's own ref and a leaving ref. Memoise the result: it must stay one function. */
export function alsoLeaving<T extends HTMLElement>(own: { current: T | null }, leaving: (el: T | null) => (() => void) | undefined) {
    return (el: T | null) => {
        own.current = el;
        const leave = leaving(el);
        return () => {
            own.current = null;
            leave?.();
        };
    };
}
