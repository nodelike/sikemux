export type TextInsertHandler = (text: string) => void;

const handlers = new Map<HTMLElement, TextInsertHandler>();

let lastFocused: HTMLElement | null = null;
let watchingFocus = false;

function watchFocus(): void {
    if (watchingFocus || typeof document === "undefined") return;
    watchingFocus = true;
    document.addEventListener("focusin", (event) => {
        const target = resolveTextInsertTarget(event.target as HTMLElement | null);
        if (target) lastFocused = target;
    });
}

function onScreen(el: HTMLElement): boolean {
    if (!el.isConnected) return false;
    const rect = el.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return false;
    return el.ownerDocument.defaultView?.getComputedStyle(el).visibility !== "hidden";
}

/** Register a surface that accepts typed text, such as a terminal or a chat composer. */
export function registerTextInsert(el: HTMLElement, fn: TextInsertHandler): () => void {
    handlers.set(el, fn);
    watchFocus();
    return () => {
        if (handlers.get(el) !== fn) return;
        handlers.delete(el);
        if (lastFocused === el) lastFocused = null;
    };
}

export function resolveTextInsertTarget(el: HTMLElement | null): HTMLElement | null {
    for (let target = el; target; target = target.parentElement) {
        if (handlers.has(target)) return target;
    }
    return null;
}

/** The surface the user is typing in, or was last typing in if focus has since moved to chrome. */
export function focusedTextInsertTarget(): HTMLElement | null {
    const active = resolveTextInsertTarget(document.activeElement as HTMLElement | null);
    if (active && onScreen(active)) return active;
    if (lastFocused && onScreen(lastFocused)) return lastFocused;
    return null;
}

/** The first surface inside `root` that accepts typed text. */
export function textInsertTargetWithin(root: HTMLElement): HTMLElement | null {
    for (const el of handlers.keys()) if (root.contains(el)) return el;
    return null;
}

export function insertText(el: HTMLElement, text: string): boolean {
    const fn = handlers.get(el);
    if (!fn) return false;
    fn(text);
    return true;
}
