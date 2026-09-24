import { groupSegments } from "../shape";

export interface GroupNode {
    name: string;
    /** Slash-joined segments from the project root, in their original case. */
    path: string;
    /** Jobs in this group and every group below it. */
    count: number;
    children: GroupNode[];
}

export interface GroupTree {
    children: GroupNode[];
    ungrouped: number;
    total: number;
}

export function buildGroupTree(jobs: { group: string | null }[]): GroupTree {
    const root: GroupNode = { name: "", path: "", count: 0, children: [] };
    let ungrouped = 0;
    for (const job of jobs) {
        const segments = groupSegments(job.group);
        root.count += 1;
        if (segments.length === 0) {
            ungrouped += 1;
            continue;
        }
        let node = root;
        for (const segment of segments) {
            let child = node.children.find((candidate) => candidate.name === segment);
            if (!child) {
                child = { name: segment, path: node.path ? `${node.path}/${segment}` : segment, count: 0, children: [] };
                node.children.push(child);
            }
            child.count += 1;
            node = child;
        }
    }
    sortTree(root);
    return { children: root.children, ungrouped, total: root.count };
}

function sortTree(node: GroupNode): void {
    node.children.sort((a, b) => a.name.localeCompare(b.name));
    node.children.forEach(sortTree);
}

/** Every folder that must be open for `path` to be visible, including `path` itself. */
export function ancestorPaths(path: string | null): string[] {
    const segments = groupSegments(path);
    return segments.map((_, index) => segments.slice(0, index + 1).join("/"));
}
