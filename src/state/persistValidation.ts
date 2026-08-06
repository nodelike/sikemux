import type { LayoutNode, PaneKind, Window, WindowRole } from "./types";

export const PERSISTED_PANE_KINDS = new Set<PaneKind>(["terminal", "editor", "git", "aws", "search", "rundeck", "bruno"]);
export const PERSISTED_WINDOW_ROLES = new Set<WindowRole>(["term", "files", "git", "search", "aws", "rundeck", "bruno", "ssh-config", "named"]);

export interface LayoutValidationLimits {
    maxDepth: number;
    maxNodes: number;
    maxChildren: number;
    maxStringLength: number;
}

export interface ValidLayout {
    root: LayoutNode;
    ids: string[];
    paneIds: string[];
}

export type LayoutValidation = { ok: true; value: ValidLayout } | { ok: false; reason: string };

export const DEFAULT_LAYOUT_LIMITS: LayoutValidationLimits = {
    maxDepth: 32,
    maxNodes: 1_024,
    maxChildren: 64,
    maxStringLength: 8_192,
};

export function isRecord(value: unknown): value is Record<string, unknown> {
    return !!value && typeof value === "object" && !Array.isArray(value);
}

function boundedString(value: unknown, maxLength: number, allowEmpty = false): value is string {
    return typeof value === "string" && value.length <= maxLength && (allowEmpty || value.length > 0);
}

/**
 * Iterative validation shared by hydration and clipboard imports. Keeping the
 * walk iterative prevents attacker-controlled nesting from overflowing the JS
 * stack before the configured depth limit can be enforced.
 */
export function validatePersistedLayout(value: unknown, limits: LayoutValidationLimits = DEFAULT_LAYOUT_LIMITS): LayoutValidation {
    const ids: string[] = [];
    const paneIds: string[] = [];
    const seen = new Set<string>();
    const pending: Array<{ value: unknown; depth: number }> = [{ value, depth: 1 }];

    while (pending.length > 0) {
        const current = pending.pop()!;
        if (current.depth > limits.maxDepth) return { ok: false, reason: `layout exceeds maximum depth (${limits.maxDepth})` };
        if (!isRecord(current.value)) return { ok: false, reason: "layout node is not an object" };
        if (!boundedString(current.value.id, limits.maxStringLength)) return { ok: false, reason: "layout node has an invalid id" };
        if (seen.has(current.value.id)) return { ok: false, reason: `duplicate layout id: ${current.value.id}` };
        seen.add(current.value.id);
        ids.push(current.value.id);
        if (ids.length > limits.maxNodes) return { ok: false, reason: `layout exceeds maximum node count (${limits.maxNodes})` };

        if (current.value.type === "pane") {
            if (!boundedString(current.value.cwd, limits.maxStringLength, true)) return { ok: false, reason: "pane has an invalid cwd" };
            if (!PERSISTED_PANE_KINDS.has(current.value.kind as PaneKind))
                return { ok: false, reason: `unsupported pane kind: ${String(current.value.kind)}` };
            if (!boundedString(current.value.title, limits.maxStringLength, true)) return { ok: false, reason: "pane has an invalid title" };
            if (current.value.startup !== undefined && !boundedString(current.value.startup, limits.maxStringLength, true))
                return { ok: false, reason: "pane startup is invalid" };
            paneIds.push(current.value.id);
            continue;
        }

        if (current.value.type !== "split") return { ok: false, reason: `unsupported layout node type: ${String(current.value.type)}` };
        if (current.value.dir !== "row" && current.value.dir !== "column") return { ok: false, reason: "split direction is invalid" };
        if (!Array.isArray(current.value.children) || current.value.children.length === 0)
            return { ok: false, reason: "split must contain children" };
        if (current.value.children.length > limits.maxChildren)
            return { ok: false, reason: `split exceeds maximum child count (${limits.maxChildren})` };
        if (!Array.isArray(current.value.sizes) || current.value.sizes.length !== current.value.children.length)
            return { ok: false, reason: "split sizes do not match its children" };
        if (!current.value.sizes.every((size) => typeof size === "number" && Number.isFinite(size) && size > 0 && size <= 1))
            return { ok: false, reason: "split sizes are invalid" };
        const total = current.value.sizes.reduce<number>((sum, size) => sum + (size as number), 0);
        if (!Number.isFinite(total) || Math.abs(total - 1) > 0.001)
            return { ok: false, reason: "split sizes must be positive fractions totaling one" };
        for (let index = current.value.children.length - 1; index >= 0; index--)
            pending.push({ value: current.value.children[index], depth: current.depth + 1 });
    }

    return { ok: true, value: { root: value as LayoutNode, ids, paneIds } };
}

export function validatePersistedWindow(
    value: unknown,
    limits: LayoutValidationLimits = DEFAULT_LAYOUT_LIMITS,
): { window: Window; layout: ValidLayout } | null {
    if (!isRecord(value)) return null;
    if (!boundedString(value.id, limits.maxStringLength) || !boundedString(value.name, limits.maxStringLength, true)) return null;
    if (!PERSISTED_WINDOW_ROLES.has(value.role as WindowRole)) return null;
    if (!boundedString(value.activePaneId, limits.maxStringLength)) return null;
    if (value.fixed !== undefined && typeof value.fixed !== "boolean") return null;
    const layout = validatePersistedLayout(value.root, limits);
    if (!layout.ok || !layout.value.paneIds.includes(value.activePaneId)) return null;
    return { window: value as unknown as Window, layout: layout.value };
}
