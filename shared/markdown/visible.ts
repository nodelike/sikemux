import type { MdElement, MdNode } from "./types";

/* A finished block keeps its object from one change to the next, so its length is worked out once. */
const lengths = new WeakMap<object, number>();

function inside(element: MdElement): readonly MdNode[] {
    if ("c" in element) return element.c;
    if (element.t === "table") return [...(element.head ?? []), ...element.rows.flat()].flat();
    return [];
}

/** How many characters of `node` a reader sees. */
export function visibleLength(node: MdNode): number {
    if (typeof node === "string") return node.length;
    const known = lengths.get(node);
    if (known !== undefined) return known;
    const length =
        node.t === "pre" || node.t === "code"
            ? node.v.length
            : node.t === "br"
              ? 1
              : inside(node).reduce((sum, child) => sum + visibleLength(child), 0);
    lengths.set(node, length);
    return length;
}

/** The characters a reader sees, in order. */
export function visibleText(nodes: readonly MdNode[]): string {
    return nodes
        .map((node) => {
            if (typeof node === "string") return node;
            if (node.t === "pre" || node.t === "code") return node.v;
            if (node.t === "br") return "\n";
            return visibleText(inside(node));
        })
        .join("");
}

function cutNode(node: MdNode, chars: number): MdNode {
    if (typeof node === "string") return node.slice(0, chars);
    if (node.t === "pre" || node.t === "code") return { ...node, v: node.v.slice(0, chars) };
    if ("c" in node) return { ...node, c: cutNodes(node.c, chars) } as MdElement;
    return node;
}

function cutNodes(nodes: readonly MdNode[], chars: number): MdNode[] {
    const kept: MdNode[] = [];
    let left = chars;
    for (const node of nodes) {
        if (left <= 0) break;
        const length = visibleLength(node);
        kept.push(length <= left ? node : cutNode(node, left));
        left -= length;
    }
    return kept;
}

/** The blocks as far as their first `chars` visible characters, keeping each whole block as the object it was. */
export function revealBlocks(blocks: readonly MdElement[], chars: number): MdElement[] {
    return cutNodes(blocks, chars) as MdElement[];
}
