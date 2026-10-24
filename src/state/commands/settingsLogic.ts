import type { ProjectRoot } from "../types";

export function normaliseProjectRoots(raw: unknown): ProjectRoot[] {
    if (!Array.isArray(raw)) return [];
    return raw
        .map((r): ProjectRoot | null => {
            if (typeof r === "string") return { path: r, depth: 1, selfIndex: false };
            if (r && typeof r === "object" && typeof (r as ProjectRoot).path === "string") {
                const depth = (r as ProjectRoot).depth;
                return {
                    path: (r as ProjectRoot).path,
                    depth: Number.isFinite(depth) ? Math.max(0, Math.round(depth)) : 1,
                    selfIndex: (r as ProjectRoot).selfIndex === true,
                };
            }
            return null;
        })
        .filter((x): x is ProjectRoot => x !== null);
}

/**
 * Fold a persisted pinned-projects list into the roots list.
 *
 * A pinned project was "index exactly this folder, repo or not", which is a
 * self-indexed root that scans nothing. Merging keeps every existing entry
 * working without asking anyone to re-add their folders.
 */
export function mergePinnedIntoRoots(roots: ProjectRoot[], pinnedRaw: unknown): ProjectRoot[] {
    if (!Array.isArray(pinnedRaw)) return roots;
    const merged = [...roots];
    for (const entry of pinnedRaw) {
        const path = typeof entry === "string" ? entry : (entry as { path?: unknown })?.path;
        if (typeof path !== "string" || !path) continue;
        const existing = merged.find((root) => root.path === path);
        if (existing) existing.selfIndex = true;
        else merged.push({ path, depth: 0, selfIndex: true });
    }
    return merged;
}
