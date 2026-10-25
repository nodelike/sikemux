import type { MdElement, MdOp } from "./types";

type Tree = unknown[];

function inside(node: unknown): Tree {
    if (Array.isArray(node)) return node;
    const element = node as { c?: Tree; rows?: Tree };
    return element.c ?? element.rows ?? [];
}

function withInside(node: unknown, list: Tree): unknown {
    if (Array.isArray(node)) return list;
    return "c" in (node as object) ? { ...(node as object), c: list } : { ...(node as object), rows: list };
}

function grown(node: unknown, op: { at: number; s: string }): unknown {
    const grow = (text: string) => text.slice(0, op.at) + op.s;
    return typeof node === "string" ? grow(node) : { ...(node as object), v: grow((node as { v: string }).v) };
}

function applied(list: Tree, path: readonly number[], op: MdOp): Tree {
    const [index, ...rest] = path;
    if (index === undefined) return "c" in op ? [...list.slice(0, op.at), ...op.c] : list;
    const copy = [...list];
    copy[index] = "s" in op && rest.length === 0 ? grown(list[index], op) : withInside(list[index], applied(inside(list[index]), rest, op));
    return copy;
}

/** `blocks` with the core's changes made, keeping every untouched node as the object it was. */
export function applyPatch(blocks: readonly MdElement[], ops: readonly MdOp[]): MdElement[] {
    let tree: Tree = blocks as Tree;
    for (const op of ops) tree = applied(tree, op.p, op);
    return tree as MdElement[];
}
