import { useMemo } from "react";
import { currentBranch, git, gitOverviewR } from "../plugin-api/host";
import { resource, useCachedResourceEnabled, useResourceEnabled } from "../plugin-api/resources";
import { enabledCodeHosts, type CodeHost } from "./registry";
import { accountForR } from "./resources";
import { pullListR } from "./lists";
import { useStore } from "../state/store";
import { refOf } from "./state";
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
        if (resolved?.repo && resolved.sameHost) {
            return { provider: host.id, owner: resolved.repo.owner, name: resolved.repo.name, host: resolved.repo.host };
        }
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
    const fromRemote = useCachedResourceEnabled(enabled && !!cwd, remoteRepoR, cwd ?? "");
    const overview = useCachedResourceEnabled(enabled && !!cwd, gitOverviewR, cwd ?? "");
    const remote = fromRemote.data ?? null;
    const provider = remote?.provider ?? "";
    const chosen = useStore((s) => (cwd && provider ? (s.codeHosts[cwd]?.[provider]?.repo ?? null) : null));
    const pickedAccount = useStore((s) => (cwd && provider ? (s.codeHosts[cwd]?.[provider]?.account ?? null) : null));
    const overridden = useMemo(() => (chosen ? refOf(provider, chosen) : null), [chosen, provider]);
    const shown = overridden ?? remote;
    const found = useCachedResourceEnabled(enabled && !!shown && !pickedAccount, accountForR, shown ?? NO_REPO);
    const account = pickedAccount ?? found.data ?? null;
    const findingAccount = !!shown && !pickedAccount && found.status === "loading" && found.data === undefined;
    // Held back until its account is known, so nothing is asked of the host as the wrong account first.
    const repo = useMemo(() => (shown && !findingAccount ? { ...shown, account } : null), [shown, account, findingAccount]);
    return {
        repo,
        remote,
        branch: currentBranch(overview.data?.status),
        loading: (!!cwd && fromRemote.status === "loading" && !fromRemote.data) || findingAccount,
    };
}

const NO_PULLS: ReadonlyMap<string, Pull> = new Map();

/** The open pull request of each of the project's own branches, by branch name, as the core lists them. */
export function useBranchPulls(cwd: string | null, enabled: boolean): ReadonlyMap<string, Pull> {
    const pulls = useResourceEnabled(enabled && !!cwd, pullListR, cwd ?? "", "open");
    return useMemo(
        () =>
            pulls.data
                ? new Map(pulls.data.pulls.filter((listed) => listed.ownBranch && listed.pull.head).map(({ pull }) => [pull.head!, pull]))
                : NO_PULLS,
        [pulls.data],
    );
}

export function pullMerged(pull: Pick<Pull, "state" | "mergedAt"> | null): boolean {
    return !!pull && (pull.state === "merged" || !!pull.mergedAt);
}

/** A draft that is still open reads as its own state, the way GitHub shows it; a closed draft reads closed. */
export function pullState(pull: Pick<Pull, "state" | "draft" | "mergedAt">): string {
    if (pullMerged(pull)) return "merged";
    return pull.draft && pull.state === "open" ? "draft" : pull.state;
}

export const PULL_STATE_LABEL: Readonly<Record<string, string>> = { open: "Open", closed: "Closed", merged: "Merged", draft: "Draft" };

const NO_PULL_LIST: readonly Pull[] = [];

/** Every pull request a branch of the project has had, in any state. */
export function useBranchPullList(cwd: string | null, branch: string, enabled: boolean): readonly Pull[] {
    const pulls = useResourceEnabled(enabled && !!cwd, pullListR, cwd ?? "", "all");
    return useMemo(
        () => pulls.data?.pulls.filter((listed) => listed.ownBranch && listed.pull.head === branch).map(({ pull }) => pull) ?? NO_PULL_LIST,
        [pulls.data, branch],
    );
}

/** A branch's pull request: the open one when there is one, or else the newest it had. */
export function useBranchPull(cwd: string | null, branch: string, enabled: boolean): Pull | null {
    const own = useBranchPullList(cwd, branch, enabled);
    return own.find((pull) => pull.state === "open") ?? own[0] ?? null;
}
