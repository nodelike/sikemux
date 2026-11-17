function prefersReducedMotion(): boolean {
  return (
    window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false
  );
}

const EASE_OUT = "cubic-bezier(0.16, 1, 0.3, 1)";
export const EASE_IN = "cubic-bezier(0.4, 0, 1, 1)";

/** Whether this element may animate now. Also false under jsdom, which has no Web Animations. */
function canAnimate(el: Element | null | undefined): el is HTMLElement {
  return (
    !!el &&
    typeof (el as HTMLElement).animate === "function" &&
    !prefersReducedMotion()
  );
}

export function animate(
  el: Element | null | undefined,
  frames: Keyframe[],
  options: KeyframeAnimationOptions,
): Animation | null {
  if (!canAnimate(el)) return null;
  return el.animate(frames, { easing: EASE_OUT, ...options });
}

/** Everything that gives a box its height, so folding it really reaches zero. */
export function foldedFrames(el: HTMLElement): [Keyframe, Keyframe] {
  const style = getComputedStyle(el);
  return [
    {
      height: `${el.offsetHeight}px`,
      paddingTop: style.paddingTop,
      paddingBottom: style.paddingBottom,
    },
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
  hooks: {
    onMount?: (el: T) => void;
    onRemove?: (el: T) => boolean | undefined;
  } = {},
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
        for (const node of [el, ...el.querySelectorAll("[id]")])
          node.removeAttribute("id");
        parent.insertBefore(
          el,
          next && next.parentNode === parent ? next : null,
        );
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
