import type { NativeViewHole } from "./nativeViews";

/* Anything that floats over the app has to show through the browser pages,
   which are native views drawn above every DOM element. Rather than each menu,
   tooltip and panel asking for a hole, this finds them all: whatever React
   portals into <body>, whatever CSS pins with `position: fixed`, and anything
   marked `data-overlay`. */

const PORTALS = "body > :not(#root, script, style, link, :has([data-browser-pane]))";
const MARKED = "[data-overlay]";
const MAX_DEPTH = 4;

function collectFixedSelectors(rules: CSSRuleList, into: Set<string>) {
    for (const rule of Array.from(rules)) {
        if (rule instanceof CSSStyleRule && rule.style.position === "fixed") into.add(rule.selectorText);
        if ("cssRules" in rule && rule.cssRules) collectFixedSelectors(rule.cssRules as CSSRuleList, into);
    }
}

function fixedSelectors(): string[] {
    const selectors = new Set<string>();
    for (const sheet of Array.from(document.styleSheets)) {
        try {
            collectFixedSelectors(sheet.cssRules, selectors);
        } catch {
            /* A stylesheet from another origin cannot be read; the app ships none. */
        }
    }
    return [...selectors].filter((selector) => {
        try {
            document.querySelector(selector);
            return true;
        } catch {
            return false;
        }
    });
}

const CLEAR = /^(transparent|rgba\(0, 0, 0, 0\))$/;

function paints(style: CSSStyleDeclaration): boolean {
    return (
        !CLEAR.test(style.backgroundColor || "transparent") ||
        (style.backgroundImage || "none") !== "none" ||
        (style.boxShadow || "none") !== "none" ||
        (style.filter || "none") !== "none" ||
        (style.backdropFilter || "none") !== "none" ||
        parseFloat(style.borderTopWidth) > 0 ||
        parseFloat(style.borderBottomWidth) > 0
    );
}

/* A see-through wrapper, like the scrim that catches a click away from a menu,
   gives up no hole of its own; the things painted inside it do. */
function holesOf(el: Element, depth: number, into: NativeViewHole[]) {
    const style = getComputedStyle(el);
    if (style.display === "none" || style.visibility === "hidden" || style.opacity === "0") return;
    const rect = el.getBoundingClientRect();
    const children = Array.from(el.children);
    const textOnly = children.length === 0 && !!el.textContent?.trim();
    if (rect.width > 0 && rect.height > 0 && (paints(style) || textOnly)) {
        into.push({
            x: Math.round(rect.left),
            y: Math.round(rect.top),
            width: Math.round(rect.width),
            height: Math.round(rect.height),
            radius: parseFloat(style.borderTopLeftRadius) || 0,
        });
        return;
    }
    if (depth >= MAX_DEPTH) return;
    for (const child of children) holesOf(child, depth + 1, into);
}

function overlaps(a: NativeViewHole, b: NativeViewHole): boolean {
    return a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
}

/* The page cuts its holes with an even-odd fill, so where two holes cross the
   page would paint again. Holes that touch become the one box around both. */
export function mergeHoles(holes: NativeViewHole[]): NativeViewHole[] {
    const merged = [...holes];
    for (let i = 0; i < merged.length; i++) {
        for (let j = i + 1; j < merged.length; j++) {
            if (!overlaps(merged[i], merged[j])) continue;
            const a = merged[i];
            const b = merged[j];
            const x = Math.min(a.x, b.x);
            const y = Math.min(a.y, b.y);
            merged[i] = {
                x,
                y,
                width: Math.max(a.x + a.width, b.x + b.width) - x,
                height: Math.max(a.y + a.height, b.y + b.height) - y,
                radius: Math.min(a.radius, b.radius),
            };
            merged.splice(j, 1);
            j = i;
        }
    }
    return merged;
}

function moving(el: Element): boolean {
    return el.getAnimations({ subtree: true }).some((animation) => {
        if (animation.playState !== "running") return false;
        return animation.effect?.getComputedTiming().iterations !== Infinity;
    });
}

/** Report the holes every floating surface needs, now and whenever they change, until stopped. */
export function watchOverlays(report: (holes: NativeViewHole[]) => void): () => void {
    const selectorNow = () => [PORTALS, MARKED, ...fixedSelectors()].join(", ");
    let selector = selectorNow();
    let frame = 0;

    const measure = () => {
        frame = 0;
        const roots = Array.from(document.querySelectorAll(selector));
        const holes: NativeViewHole[] = [];
        for (const root of roots) {
            if (roots.some((other) => other !== root && other.contains(root))) continue;
            holesOf(root, 0, holes);
        }
        report(mergeHoles(holes));
        if (roots.some(moving)) schedule();
    };
    const schedule = () => {
        if (!frame) frame = requestAnimationFrame(measure);
    };

    const observer = new MutationObserver((records) => {
        if (records.some((record) => record.target === document.head || record.target.parentNode === document.head)) {
            selector = selectorNow();
        }
        schedule();
    });
    const restyled = (event: Event) => {
        if (!(event.target instanceof HTMLLinkElement)) return;
        selector = selectorNow();
        schedule();
    };
    document.addEventListener("load", restyled, true);
    observer.observe(document.documentElement, {
        subtree: true,
        childList: true,
        attributes: true,
        attributeFilter: ["class", "style", "hidden", "open", "data-overlay"],
    });
    window.addEventListener("resize", schedule);
    window.addEventListener("scroll", schedule, { capture: true, passive: true });
    window.addEventListener("animationstart", schedule, true);
    window.addEventListener("transitionrun", schedule, true);
    measure();

    return () => {
        observer.disconnect();
        document.removeEventListener("load", restyled, true);
        window.removeEventListener("resize", schedule);
        window.removeEventListener("scroll", schedule, { capture: true });
        window.removeEventListener("animationstart", schedule, true);
        window.removeEventListener("transitionrun", schedule, true);
        if (frame) cancelAnimationFrame(frame);
        report([]);
    };
}
