import type { KeyboardEvent } from "react";

export function navigateTabs(event: KeyboardEvent<HTMLElement>): void {
    const list = event.currentTarget.closest('[role="tablist"]');
    /*
     * A vertical tablist — the workspace rail's icon gutter — walks on the
     * up/down arrows; a horizontal strip keeps left/right. The orientation is
     * read off the list rather than passed in, so a tab stays identical
     * wherever it is mounted.
     */
    const axis =
        list?.getAttribute("aria-orientation") === "vertical" ? { prev: "ArrowUp", next: "ArrowDown" } : { prev: "ArrowLeft", next: "ArrowRight" };
    const keys: string[] = [axis.prev, axis.next, "Home", "End"];
    if (!keys.includes(event.key)) return;
    const tabs = [...(list?.querySelectorAll<HTMLButtonElement>('[role="tab"]:not(:disabled)') ?? [])];
    const index = tabs.indexOf(event.currentTarget as HTMLButtonElement);
    if (index < 0 || !tabs.length) return;
    event.preventDefault();
    const next =
        event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : (index + (event.key === axis.next ? 1 : -1) + tabs.length) % tabs.length;
    tabs[next].focus();
    tabs[next].click();
}
