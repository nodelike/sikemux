import type { MdNode } from "./types";

const MARKS =
    /`([^`\n]+)`|\*\*(.+?)\*\*|__(.+?)__|\[([^\]\n]+)\]\(([^)\s]+)\)|\*([^*\s](?:[^*\n]*[^*\s])?)\*|(?<![\w])_([^_\s](?:[^_\n]*[^_\s])?)_(?![\w])/g;

/** One line of a call's words as the tree the transcript draws: code, bold, italics and links, the rest as written. */
export function inlineNodes(text: string): MdNode[] {
    const nodes: MdNode[] = [];
    let at = 0;
    for (const found of text.matchAll(MARKS)) {
        const start = found.index ?? 0;
        if (start > at) nodes.push(text.slice(at, start));
        const [, code, strong, under, label, href, em, underEm] = found;
        if (code !== undefined) nodes.push({ t: "code", v: code });
        else if (strong !== undefined || under !== undefined) nodes.push({ t: "strong", c: inlineNodes(strong ?? under ?? "") });
        else if (label !== undefined && href !== undefined) nodes.push({ t: "a", href, c: inlineNodes(label) });
        else nodes.push({ t: "em", c: inlineNodes(em ?? underEm ?? "") });
        at = start + found[0].length;
    }
    if (at < text.length) nodes.push(text.slice(at));
    return nodes;
}
