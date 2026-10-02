import { git } from "../api/git";
import { runGitCmd } from "../state/git";
import type { CodeHost } from "./registry";
import type { Pull, RepoRef } from "./types";

/** Whether a pull request's branch lives on the repository itself rather than on a fork of it. */
export function isOwnBranch(pull: Pick<Pull, "head" | "headLabel">, repo: RepoRef): boolean {
    if (!pull.head) return false;
    const owner = pull.headLabel?.split(":")[0];
    return !owner || owner === repo.owner;
}

/** The local branch a pull request is checked out onto. */
export function localBranchOf(pull: Pick<Pull, "number" | "head" | "headLabel">, repo: RepoRef): string | null {
    if (isOwnBranch(pull, repo)) return pull.head;
    return `pr-${pull.number}`;
}

/**
 * Checks a pull request out in the project folder: its own branch when it lives on the repository, or the host's ref
 * for it when it comes from a fork. The git pane's checkout keeps uncommitted work safe either way.
 */
export async function checkoutPull(cwd: string, host: CodeHost, repo: RepoRef, pull: Pull): Promise<string> {
    const label = `Check out #${pull.number}`;
    if (isOwnBranch(pull, repo) && pull.head) {
        const head = pull.head;
        return runGitCmd(
            label,
            async () => {
                await git.fetch(cwd, "origin");
                const branches = await git.branches(cwd);
                if (branches.some((branch) => branch.name === head)) return git.checkoutSmart(cwd, head);
                await git.checkoutRemoteBranch(cwd, "origin", head, head);
                return `Checked out ${head}`;
            },
            { repo: cwd },
        );
    }
    const ref = host.pullHeadRef?.(pull.number);
    if (!ref) throw new Error(`${host.name} has no way to fetch a pull request from a fork`);
    const local = `pr-${pull.number}`;
    return runGitCmd(
        label,
        async () => {
            await git.fetchRef(cwd, "origin", ref, local);
            return git.checkoutSmart(cwd, local);
        },
        { repo: cwd },
    );
}
