import { getState, mutate, setState, type StoreState } from "../store";
import { DEFAULT_GIT_VIEW, type GitArea } from "../types";
import { collectPanes } from "../layout";
import type { DiffTarget } from "../types";
import { ensureRoleWindow } from "./shared";

export const openDiffPane = (): void => ensureRoleWindow("diff", "diff", "diff");

export const openGitWorkbench = (): void => ensureRoleWindow("git", "git", "Git");

export function openGitPane(): void {
    openGitWorkbench();
}

/** The git pane in the project in front, opening one when there is none. */
export function gitPaneOfActiveSession(): string | null {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session || session.kind !== "project") return null;
    for (const id of st.windowsBySession[session.id] ?? []) {
        const pane = st.windows[id] && collectPanes(st.windows[id].root).find((candidate) => candidate.kind === "git");
        if (pane) return pane.id;
    }
    return null;
}

/** Opens the git pane at its local workbench or at one of the code host's sections, and says which pane that is. */
export function openGitArea(area: GitArea): string | null {
    openGitWorkbench();
    const paneId = gitPaneOfActiveSession();
    if (paneId) setGitView(paneId, { area });
    return paneId;
}

function focusDiff(target: DiffTarget): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session) return;
    setState((state) => ({ diffTarget: { ...state.diffTarget, [session.cwd]: target } }));
    ensureRoleWindow("diff", "diff", "diff");
}

/** Review one changed file in the diff tab. `path` is repo-relative. */
export const openDiff = (path: string): void => focusDiff({ kind: "worktree", path });

/** Review a whole commit in the diff tab. */
export const openCommitDiff = (rev: string, subject: string): void => focusDiff({ kind: "commit", rev, subject });

export function setGitView(paneId: string, patch: Partial<StoreState["gitViews"][string]>): void {
    mutate((d) => {
        const cur = (d.gitViews[paneId] ?? DEFAULT_GIT_VIEW) as StoreState["gitViews"][string];
        d.gitViews[paneId] = { ...cur, ...patch };
    });
}
