/** Runs `work` once the page has nothing else to do, so fetching it never competes with drawing. */
export function whenIdle(work: () => void): () => void {
    if (typeof window.requestIdleCallback === "function") {
        const id = window.requestIdleCallback(work, { timeout: 2000 });
        return () => window.cancelIdleCallback(id);
    }
    const timer = window.setTimeout(work, 200);
    return () => window.clearTimeout(timer);
}
