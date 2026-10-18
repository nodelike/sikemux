/** Reads what a terminal shows, by the element that holds it. */
type ScreenReader = () => string[];

const readers = new Map<HTMLElement, ScreenReader>();

export function registerScreen(el: HTMLElement, read: ScreenReader): () => void {
    readers.set(el, read);
    return () => {
        if (readers.get(el) === read) readers.delete(el);
    };
}

/** The rows on screen of the first terminal inside `root`, or none. */
export function screenWithin(root: HTMLElement): string[] {
    for (const [el, read] of readers) if (root.contains(el)) return read();
    return [];
}
