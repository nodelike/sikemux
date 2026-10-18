import type { DiscoveredRepo } from "../api/git";
import { useCachedResourceEnabled } from "../state/resources";
import { gitDiscoveredReposR, gitOverviewR } from "../state/resources.defs";
import { useStore } from "../state/store";

export function isMissingRepository(error: string | null | undefined): boolean {
    return !!error && /could not find repository/i.test(error);
}

/** The person's chosen repository while it is still there, or else the only one, when a folder holds several. */
export function defaultRepoAmong(chosen: string | undefined, repos: readonly DiscoveredRepo[]): string | null {
    if (chosen && repos.some((repo) => repo.path === chosen)) return chosen;
    return repos.length === 1 ? repos[0].path : null;
}

export interface ProjectRepo {
    /** The repository the project's Git views open; null while unknown or when it is the person's to pick. */
    readonly repo: string | null;
    /** The repositories directly inside, when the project folder is not one itself. */
    readonly repos: readonly DiscoveredRepo[] | null;
}

/**
 * The repository a project's Git views use: the folder itself when it is one,
 * else the default the person chose among the repositories inside it, else the
 * only one there is.
 */
export function useProjectRepo(cwd: string, enabled = true): ProjectRepo {
    const overview = useCachedResourceEnabled(enabled && !!cwd, gitOverviewR, cwd);
    const missing = overview.status === "error" && !overview.data && isMissingRepository(overview.error);
    const discovered = useCachedResourceEnabled(enabled && missing, gitDiscoveredReposR, cwd);
    const chosen = useStore((s) => s.defaultGitRepos[cwd]);
    if (!missing) return { repo: cwd || null, repos: null };
    const repos = discovered.data ?? null;
    return { repo: repos ? defaultRepoAmong(chosen, repos) : (chosen ?? null), repos };
}
