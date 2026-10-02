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
    | { t: "fnref"; n: number };

export interface MarkdownOptions {
    /** Tables, strikethrough, task lists, footnotes and bare addresses as links. */
    readonly gfm: boolean;
    /** Shows markup as the characters that were typed instead of dropping it. */
    readonly htmlAsText: boolean;
    /** Keeps `file://` and drive-letter links, which are emptied otherwise. */
    readonly fileLinks: boolean;
    /** Keeps the pictures in embedded `<img>` tags while the rest of the markup is dropped. */
    readonly htmlImages?: boolean;
}
