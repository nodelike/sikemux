export type BranchKind = "main" | "feature" | "fix" | "release" | "other" | "na";

export function branchKind(branch: string | null | undefined): BranchKind {
    if (!branch || branch === "N/A" || branch === "·") return "na";
    const lower = branch.toLowerCase();
    if (lower === "main" || lower === "master" || lower.endsWith("/main") || lower.endsWith("/master")) return "main";
    if (lower.startsWith("feature/") || lower.startsWith("feat/") || lower.includes("/feature/")) return "feature";
    if (lower.startsWith("fix/") || lower.startsWith("hotfix/") || lower.startsWith("bugfix/")) return "fix";
    if (lower.startsWith("release/")) return "release";
    if (lower.startsWith("rewrite/") || lower.startsWith("enhance/") || lower.startsWith("refactor/")) return "feature";
    return "other";
}

export const BRANCH_GLYPH: Record<BranchKind, string> = {
    main: "●",
    feature: "◆",
    fix: "▲",
    release: "✦",
    other: "◇",
    na: "·",
};

export function statusKind(status: string | null | undefined): "succeeded" | "failed" | "running" | "aborted" | "unknown" {
    if (!status) return "unknown";
    const s = status.toLowerCase();
    if (s === "succeeded") return "succeeded";
    if (s === "failed" || s === "failed-with-retry" || s === "timedout" || s === "other-failed" || s === "error") return "failed";
    if (s === "running" || s === "scheduled" || s === "queued") return "running";
    if (s === "aborted") return "aborted";
    return "unknown";
}
