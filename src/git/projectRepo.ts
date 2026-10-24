import type { ProjectRepo } from "../api/git";
import { onCoreChanged } from "../api/coreChanged";
import { invalidate, useCachedResourceEnabled } from "../state/resources";
import { gitProjectRepoR } from "../state/resources.defs";

export type { ProjectRepo } from "../api/git";

let following = false;

/** The person's default repository is a preference, so the core's pick is read again whenever preferences change. */
function follow(): void {
    if (following) return;
    following = true;
    onCoreChanged("preferences", () => invalidate((kind) => kind === gitProjectRepoR.kind));
}

const UNKNOWN: ProjectRepo = { repo: null, repos: null };

/**
 * The repository a project's Git views use, as the core picks it: the folder itself
 * when it is one, else the default the person chose among the repositories inside
 * it, else the only one there is.
 */
export function useProjectRepo(cwd: string, enabled = true): ProjectRepo {
    follow();
    return useCachedResourceEnabled(enabled && !!cwd, gitProjectRepoR, cwd).data ?? UNKNOWN;
}
