import { useEffect, useState } from "react";
import { markdownApi } from "../api/markdown";
import { swallow } from "../state/toast";
import type { MarkdownOptions, MdElement } from "./types";

/* A finished message is read once. A transcript scrolled back and forth mounts
   the same messages again, and they should draw in the frame they mount. */
const REMEMBERED_PER_KIND = 256;
const remembered = new Map<string, Map<string, MdElement[]>>();

function kindOf(options: MarkdownOptions): string {
    return `${+options.gfm}${+options.htmlAsText}${+options.fileLinks}`;
}

function recall(options: MarkdownOptions, text: string): MdElement[] | undefined {
    return remembered.get(kindOf(options))?.get(text);
}

function remember(options: MarkdownOptions, text: string, blocks: MdElement[]): void {
    const kind = kindOf(options);
    let shelf = remembered.get(kind);
    if (!shelf) remembered.set(kind, (shelf = new Map()));
    shelf.delete(text);
    shelf.set(text, blocks);
    if (shelf.size > REMEMBERED_PER_KIND) shelf.delete(shelf.keys().next().value!);
}

export function forgetMarkdownForTests(): void {
    remembered.clear();
}

function sameNode(a: unknown, b: unknown): boolean {
    if (a === b) return true;
    if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
    if (Array.isArray(a)) return Array.isArray(b) && a.length === b.length && a.every((item, index) => sameNode(item, b[index]));
    const keys = Object.keys(a);
    if (keys.length !== Object.keys(b).length) return false;
    return keys.every((key) => sameNode((a as Record<string, unknown>)[key], (b as Record<string, unknown>)[key]));
}

/** Keeps every block that came back unchanged as the object already drawn, so it is not drawn again. */
export function settleBlocks(previous: readonly MdElement[] | undefined, skip: number, tail: readonly MdElement[]): MdElement[] {
    const blocks = previous ? previous.slice(0, skip) : [];
    tail.forEach((block, index) => {
        const old = previous?.[skip + index];
        blocks.push(old && sameNode(old, block) ? old : block);
    });
    return blocks;
}

/* The line being written may still join the block before it, as a table row
   or a heading's underline does, so the last two blocks are read again. */
const UNSETTLED_BLOCKS = 2;

interface Wanted {
    readonly text: string;
    readonly options: MarkdownOptions;
    readonly live: boolean;
}

interface Read extends Wanted {
    readonly blocks: MdElement[];
}

/* One read at a time per message. Text that arrives meanwhile waits for the
   next frame and is read as a whole, so a fast stream costs a read a frame. */
class Reader {
    alive = true;
    private held: Read | null = null;
    private wanted: Wanted | null = null;
    private busy = false;

    constructor(private readonly show: (read: Read) => void) {}

    want(wanted: Wanted): void {
        this.wanted = wanted;
        if (!this.busy) this.next();
    }

    private next(): void {
        const wanted = this.wanted;
        if (!wanted) return;
        this.wanted = null;
        const previous = this.held;
        const growing = wanted.live && previous?.options === wanted.options && wanted.text.startsWith(previous.text);
        const skip = growing ? Math.max(0, previous.blocks.length - UNSETTLED_BLOCKS) : 0;
        this.busy = true;
        markdownApi
            .parse({ text: wanted.text, options: wanted.options, skip })
            .then(
                (tail) => settleBlocks(previous?.blocks, skip, tail),
                (error: unknown): MdElement[] => {
                    swallow("read markdown")(error);
                    return [{ t: "p", c: [wanted.text] }];
                },
            )
            .then((blocks) => {
                if (!wanted.live) remember(wanted.options, wanted.text, blocks);
                this.held = { ...wanted, blocks };
                this.busy = false;
                if (this.alive) this.show(this.held);
                if (this.wanted)
                    requestAnimationFrame(() => {
                        if (!this.busy) this.next();
                    });
            });
    }
}

/**
 * The blocks `text` reads as, or null until the first read comes back. While
 * `live`, the message is still being written and only its end is read again.
 */
export function useMarkdownBlocks(text: string, options: MarkdownOptions, live: boolean): readonly MdElement[] | null {
    const known = recall(options, text);
    const [read, setRead] = useState<Read | null>(null);
    const [reader] = useState(() => new Reader(setRead));
    useEffect(() => {
        reader.alive = true;
        return () => {
            reader.alive = false;
        };
    }, [reader]);
    useEffect(() => {
        if (!known) reader.want({ text, options, live });
    }, [reader, known, text, options, live]);
    return known ?? read?.blocks ?? null;
}
