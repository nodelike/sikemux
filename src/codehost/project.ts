import { useMemo } from "react";
import { git, gitOverviewR } from "../plugin-api/host";
import { resource, useResourceEnabled } from "../plugin-api/resources";
import { isOwnBranch } from "./checkout";
import { enabledCodeHosts, type CodeHost } from "./registry";
import { accountForR, hostStatusR, pullsR } from "./resources";
import { hostSettings, refOf } from "./state";
import type { Pull, RepoRef } from "./types";

/** `origin` is what people push to, so it is the remote that names the repository. */
export function pickRemote(remotes: readonly { name: string; url: string }[]): string | null {
    const origin = remotes.find((remote) => remote.name === "origin");
    return (origin ?? remotes[0])?.url ?? null;
}

/**
 * The repository a folder's remote points at, on the first enabled host that serves it. A host describes any remote it
 * can parse, but claims only one on its own server: a Bitbucket or GitLab repository is not GitHub's to show.
 */
export const remoteRepoR = resource({
    kind: "host.remoteRepo",
    fetch: async (cwd: string): Promise<RepoRef | null> => {
        const url = pickRemote(await git.remotes(cwd));
        return url ? claimRemote(url, enabledCodeHosts()) : null;
    },
    staleAfterMs: 5 * 60_000,
});

export async function claimRemote(url: string, hosts: readonly Pick<CodeHost, "id" | "api">[]): Promise<RepoRef | null> {
    for (const host of hosts) {
        const resolved = await host.api.resolveRemote(url).catch(() => null);
        if (resolved?.repo && resolved.sameHost) return { provider: host.id, owner: resolved.repo.owner, name: resolved.repo.name };
    }
    return null;
}

export interface HostRepo {
    /** The repository the host sections show, which a hand-picked one for this folder overrides. */
    repo: RepoRef | null;
    /** The one the folder's remote points at, before any hand-picked override. */
    remote: RepoRef | null;
    branch: string | null;
    /** True while the folder's remote is still being read, so nothing has been ruled out yet. */
    loading: boolean;
}

const NO_REPO: RepoRef = { provider: "", owner: "", name: "" };

export function useHostRepo(cwd: string | null, enabled: boolean): HostRepo {
    const fromRemote = useResourceEnabled(enabled && !!cwd, remoteRepoR, cwd ?? "");
    const overview = useResourceEnabled(enabled && !!cwd, gitOverviewR, cwd ?? "");
    const remote = fromRemote.data ?? null;
    const provider = remote?.provider ?? "";
    const chosen = hostSettings(provider).useSelect((settings) => (cwd && provider ? (settings.repoByProject[cwd] ?? null) : null));
    const pickedAccount = hostSettings(provider).useSelect((settings) => (cwd && provider ? (settings.accountByProject[cwd] ?? null) : null));
    const overridden = useMemo(() => (chosen ? refOf(provider, chosen) : null), [chosen, provider]);
    const shown = overridden ?? remote;
    const found = useResourceEnabled(enabled && !!shown && !pickedAccount, accountForR, shown ?? NO_REPO);
    const account = pickedAccount ?? found.data ?? null;
    const findingAccount = !!shown && !pickedAccount && found.status === "loading" && found.data === undefined;
    // Held back until its account is known, so nothing is asked of the host as the wrong account first.
    const repo = useMemo(() => (shown && !findingAccount ? { ...shown, account } : null), [shown, account, findingAccount]);
    return {
        repo,
        remote,
        branch: overview.data?.status.branch ?? null,
        loading: (!!cwd && fromRemote.status === "loading" && !fromRemote.data) || findingAccount,
    };
}

const NO_PULLS: ReadonlyMap<string, Pull> = new Map();

/** The open pull request of each of the repository's own branches, by branch name, once someone is signed in to its host. */
export function useBranchPulls(repo: RepoRef | null, enabled: boolean): ReadonlyMap<string, Pull> {
    const status = useResourceEnabled(enabled && !!repo, hostStatusR, repo?.provider ?? "", repo?.account ?? null);
    const pulls = useResourceEnabled(enabled && !!repo && !!status.data?.ok, pullsR, repo ?? { provider: "", owner: "", name: "" }, "open");
    return useMemo(() => (repo && pulls.data ? pullsByBranch(pulls.data, repo) : NO_PULLS), [repo, pulls.data]);
}

/** A fork's pull request is left out: its branch of the same name is not the one in this repository. */
export function pullsByBranch(pulls: readonly Pull[], repo: RepoRef): ReadonlyMap<string, Pull> {
    const byBranch = new Map<string, Pull>();
    for (const pull of pulls) {
        if (pull.head && isOwnBranch(pull, repo)) byBranch.set(pull.head, pull);
    }
    return byBranch;
}

/** A branch's pull request: the open one when there is one, or else the newest it had. */
export function pullForBranch(pulls: readonly Pull[], repo: RepoRef, branch: string): Pull | null {
    const own = pulls.filter((pull) => pull.head === branch && isOwnBranch(pull, repo));
    return own.find((pull) => pull.state === "open") ?? own[0] ?? null;
}

export function pullMerged(pull: Pick<Pull, "state" | "mergedAt"> | null): boolean {
    return !!pull && (pull.state === "merged" || !!pull.mergedAt);
}

/** One branch's pull request in any state, once someone is signed in to its host. */
export function useBranchPull(repo: RepoRef | null, branch: string, enabled: boolean): Pull | null {
    const status = useResourceEnabled(enabled && !!repo, hostStatusR, repo?.provider ?? "", repo?.account ?? null);
    const pulls = useResourceEnabled(enabled && !!repo && !!status.data?.ok, pullsR, repo ?? NO_REPO, "all");
    return useMemo(() => (repo && pulls.data ? pullForBranch(pulls.data, repo, branch) : null), [repo, pulls.data, branch]);
}
