import type { AgentWorktree } from "../state/types";

export type WorktreeSwitchState =
    { kind: "hidden" } | { kind: "choosing"; on: boolean } | { kind: "preparing"; step: string } | { kind: "in"; branch: string; path: string };

export function worktreeSwitchState({
    worktree,
    isRepo,
    started,
    preparing,
    on,
}: {
    worktree: AgentWorktree | undefined;
    isRepo: boolean | null;
    /** The chat has a turn or a saved session to come back to. */
    started: boolean;
    preparing: string | null;
    on: boolean;
}): WorktreeSwitchState {
    if (preparing !== null) return { kind: "preparing", step: preparing };
    if (worktree) return { kind: "in", branch: worktree.branch, path: worktree.path };
    if (!isRepo || started) return { kind: "hidden" };
    return { kind: "choosing", on };
}
