import type { Divider, FocusDir, LayoutNode, PaneKind, PaneNode, Rect, SplitDir, SplitNode } from "./types";

const MIN_FRAC = 0.05; // a pane can't shrink below 5% of its split axis

const NONCE = Math.random().toString(36).slice(2, 8);
let counter = 0;
export function newId(prefix: string): string {
    return `${prefix}-${NONCE}-${Date.now().toString(36)}-${(counter++).toString(36)}`;
}

export function makePane(cwd = "", opts: { kind?: PaneKind; startup?: string } = {}): PaneNode {
    const kind = opts.kind ?? "terminal";
    const title =
        kind === "editor"
            ? "editor"
            : kind === "git"
              ? "git"
              : kind === "search"
                ? "search"
                : kind === "aws"
                  ? "aws"
                  : kind === "bruno"
                    ? "bruno"
                    : opts.startup || "shell";
    return {
        type: "pane",
        id: newId("pane"),
        cwd,
        kind,
        startup: opts.startup,
        title,
    };
}

export function collectPanes(node: LayoutNode): PaneNode[] {
    return node.type === "pane" ? [node] : node.children.flatMap(collectPanes);
}

export function findSplit(node: LayoutNode, id: string): SplitNode | null {
    if (node.type === "pane") return null;
    if (node.id === id) return node;
    for (const c of node.children) {
        const r = findSplit(c, id);
        if (r) return r;
    }
    return null;
}

export function splitPane(root: LayoutNode, paneId: string, dir: SplitDir, newPane: PaneNode): LayoutNode {
    function rec(node: LayoutNode): LayoutNode {
        if (node.type === "pane") {
            if (node.id !== paneId) return node;
            return {
                type: "split",
                id: newId("split"),
                dir,
                children: [node, newPane],
                sizes: [0.5, 0.5],
            };
        }
        const idx = node.children.findIndex((c) => c.type === "pane" && c.id === paneId);
        if (idx >= 0 && node.dir === dir) {
            const children = node.children.slice();
            const sizes = node.sizes.slice();
            const half = sizes[idx] / 2;
            sizes[idx] = half;
            children.splice(idx + 1, 0, newPane);
            sizes.splice(idx + 1, 0, half);
            return { ...node, children, sizes };
        }
        return { ...node, children: node.children.map(rec) };
    }
    return rec(root);
}

export function replacePane(root: LayoutNode, paneId: string, newPane: PaneNode): LayoutNode {
    if (root.type === "pane") return root.id === paneId ? newPane : root;
    return { ...root, children: root.children.map((child) => replacePane(child, paneId, newPane)) };
}

export function cloneLayout(root: LayoutNode): LayoutNode {
    if (root.type === "pane") return { ...root, id: newId("pane") };
    return { ...root, id: newId("split"), children: root.children.map(cloneLayout), sizes: root.sizes.slice() };
}

export function removePane(root: LayoutNode, paneId: string): LayoutNode | null {
    if (root.type === "pane") return root.id === paneId ? null : root;
    function rec(node: SplitNode): LayoutNode {
        const idx = node.children.findIndex((c) => c.type === "pane" && c.id === paneId);
        if (idx >= 0) {
            const children = node.children.slice();
            const sizes = node.sizes.slice();
            const freed = sizes[idx];
            children.splice(idx, 1);
            sizes.splice(idx, 1);
            const rest = 1 - freed || 1;
            const norm = sizes.map((s) => s / rest);
            return children.length === 1 ? children[0] : { ...node, children, sizes: norm };
        }
        return {
            ...node,
            children: node.children.map((c) => (c.type === "split" ? rec(c) : c)),
        };
    }
    return rec(root);
}

export function setSplitSizes(root: LayoutNode, splitId: string, sizes: number[]): LayoutNode {
    if (root.type === "pane") return root;
    if (root.id === splitId) return { ...root, sizes };
    return {
        ...root,
        children: root.children.map((c) => setSplitSizes(c, splitId, sizes)),
    };
}

function pathToPane(root: LayoutNode, paneId: string): { split: SplitNode; index: number }[] | null {
    const path: { split: SplitNode; index: number }[] = [];
    function rec(node: LayoutNode): boolean {
        if (node.type === "pane") return node.id === paneId;
        for (let i = 0; i < node.children.length; i++) {
            if (rec(node.children[i])) {
                path.unshift({ split: node, index: i });
                return true;
            }
        }
        return false;
    }
    return rec(root) ? path : null;
}

export function computeLayout(root: LayoutNode): {
    panes: Map<string, Rect>;
    dividers: Divider[];
} {
    const panes = new Map<string, Rect>();
    const dividers: Divider[] = [];
    function walk(node: LayoutNode, rect: Rect): void {
        if (node.type === "pane") {
            panes.set(node.id, rect);
            return;
        }
        let off = 0;
        node.children.forEach((child, i) => {
            const frac = node.sizes[i];
            const childRect: Rect =
                node.dir === "row"
                    ? { x: rect.x + off * rect.w, y: rect.y, w: frac * rect.w, h: rect.h }
                    : { x: rect.x, y: rect.y + off * rect.h, w: rect.w, h: frac * rect.h };
            walk(child, childRect);
            off += frac;
            if (i < node.children.length - 1) {
                dividers.push({ splitId: node.id, index: i, dir: node.dir, rect, at: off });
            }
        });
    }
    walk(root, { x: 0, y: 0, w: 1, h: 1 });
    return { panes, dividers };
}

export function neighborPane(panes: Map<string, Rect>, activeId: string, dir: FocusDir): string | null {
    const a = panes.get(activeId);
    if (!a) return null;
    const acx = a.x + a.w / 2;
    const acy = a.y + a.h / 2;
    let best: string | null = null;
    let bestScore = Infinity;
    for (const [id, r] of panes) {
        if (id === activeId) continue;
        const vOverlap = r.y < a.y + a.h - 1e-6 && r.y + r.h > a.y + 1e-6;
        const hOverlap = r.x < a.x + a.w - 1e-6 && r.x + r.w > a.x + 1e-6;
        let ok = false;
        let dist = 0;
        let cross = 0;
        if (dir === "left") {
            ok = vOverlap && r.x + r.w <= a.x + 1e-3;
            dist = a.x - (r.x + r.w);
            cross = Math.abs(r.y + r.h / 2 - acy);
        } else if (dir === "right") {
            ok = vOverlap && r.x >= a.x + a.w - 1e-3;
            dist = r.x - (a.x + a.w);
            cross = Math.abs(r.y + r.h / 2 - acy);
        } else if (dir === "up") {
            ok = hOverlap && r.y + r.h <= a.y + 1e-3;
            dist = a.y - (r.y + r.h);
            cross = Math.abs(r.x + r.w / 2 - acx);
        } else {
            ok = hOverlap && r.y >= a.y + a.h - 1e-3;
            dist = r.y - (a.y + a.h);
            cross = Math.abs(r.x + r.w / 2 - acx);
        }
        if (!ok) continue;
        const score = dist + cross * 0.5;
        if (score < bestScore) {
            bestScore = score;
            best = id;
        }
    }
    return best;
}

export function resizeTowards(root: LayoutNode, paneId: string, dir: FocusDir, step = 0.04): LayoutNode {
    const path = pathToPane(root, paneId);
    if (!path) return root;
    const axis: SplitDir = dir === "left" || dir === "right" ? "row" : "column";
    let target: { split: SplitNode; index: number } | null = null;
    for (let i = path.length - 1; i >= 0; i--) {
        if (path[i].split.dir === axis) {
            target = path[i];
            break;
        }
    }
    if (!target) return root;
    const { split, index } = target;
    const sib = dir === "right" || dir === "down" ? index + 1 : index - 1;
    if (sib < 0 || sib >= split.sizes.length) return root;
    const sizes = split.sizes.slice();
    const give = Math.min(step, sizes[sib] - MIN_FRAC);
    if (give <= 0) return root;
    sizes[index] += give;
    sizes[sib] -= give;
    return setSplitSizes(root, split.id, sizes);
}

export { MIN_FRAC };
