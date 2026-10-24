/* The tree `sikemux-markdown` reads a message into. A bare string is text. */

export type MdNode = string | MdElement;

export type MdAlign = "left" | "center" | "right" | null;

export type MdElement =
    | { t: "p" | "quote" | "ul" | "em" | "strong" | "del"; c: MdNode[] }
    | { t: "h"; l: 1 | 2 | 3 | 4 | 5 | 6; c: MdNode[] }
    | { t: "ol"; start?: number; c: MdNode[] }
    | { t: "li"; checked?: boolean; c: MdNode[] }
    | { t: "pre"; lang?: string; v: string }
    | { t: "hr" }
    | { t: "br" }
    | { t: "table"; align: MdAlign[]; head?: MdNode[][]; rows: MdNode[][][] }
    /** Footnotes are numbered in the order they are first mentioned. */
    | { t: "fndef"; n: number; c: MdNode[] }
    | { t: "code"; v: string }
    | { t: "a"; href: string; title?: string; c: MdNode[] }
    | { t: "img"; src: string; alt: string; title?: string }
    | { t: "fnref"; n: number }
    /** A name that could be a file, holding the words as written, between backticks when `code`. */
    | { t: "path"; href: string; code?: boolean; c: MdNode[] }
    /** A `<details>` fold from embedded HTML. */
    | { t: "details"; summary: string; open?: boolean; c: MdNode[] };

/**
 * One change to a tree: `p` is a path of indexes from the top-level blocks down
 * through each element's children (`c`, or a table's `rows`, its cells and their
 * contents). Positions in text count UTF-16 units.
 */
export type MdOp =
    /** The list at `p` keeps its first `at` entries and gains `c`. */
    | { p: number[]; at: number; c: unknown[] }
    /** The text at `p`, or a code block's `v` there, keeps its first `at` units and gains `s`. */
    | { p: number[]; at: number; s: string };

/** What the core sends with a piece of an answer: the answer's length once it is added, and how its tree changed. */
export interface MdChange {
    n: number;
    o: MdOp[];
}

export interface MarkdownOptions {
    /** Tables, strikethrough, task lists, footnotes and bare addresses as links. */
    readonly gfm: boolean;
    /** Shows markup as the characters that were typed instead of dropping it. */
    readonly htmlAsText: boolean;
    /** Keeps `file://` and drive-letter links, which are emptied otherwise. */
    readonly fileLinks: boolean;
    /** Keeps the pictures in embedded `<img>` tags while the rest of the markup is dropped. */
    readonly htmlImages?: boolean;
    /** Marks the words outside links and code blocks that name a file, as `path` nodes. */
    readonly filePaths?: boolean;
}
