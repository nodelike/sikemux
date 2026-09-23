import { Channel } from "@tauri-apps/api/core";
import { invokeCommand as invoke } from "./invoke";
import { subscribe } from "../state/bus";

export interface GitFile {
    path: string;
    index: string;
    worktree: string;
}

export interface GitStatus {
    branch: string;
    upstream: string | null;
    ahead: number;
    behind: number;
    files: GitFile[];
}

export interface DiscoveredRepo {
    path: string;
    name: string;
    branch: string;
    ahead: number;
    behind: number;
    changes: number;
}

export interface GitBranch {
    name: string;
    current: boolean;
    upstream: string | null;
}

export interface GitWorktree {
    path: string;
    head: string | null;
    branch: string | null;
    reference: string | null;
    detached: boolean;
    locked: boolean;
    lock_reason: string | null;
    prunable: boolean;
    prune_reason: string | null;
    bare: boolean;
    current: boolean;
    is_main: boolean;
}

export interface CreateGitWorktreeOptions {
    path: string;
    branch: string;
    createBranch: boolean;
    startPoint?: string | null;
}

export interface GitCommit {
    hash: string;
    full_hash: string;
    parents: string[];
    author: string;
    author_email: string;
    date: string;
    subject: string;
    refs: string[];
    unpushed: boolean;
}

export interface GitOverview {
    status: GitStatus;
    branches: GitBranch[];
    log: GitCommit[];
}

export interface GitStash {
    index: number;
    sha: string;
    refname: string;
    branch: string;
    message: string;
}

export type DiscardMode = "unstaged" | "staged" | "all";
export type StashMode = "all" | "staged" | "unstaged";
export type RepoWatchLeaseToken = string;

export interface GitRemote {
    name: string;
    url: string;
}

export interface GitRemoteBranch {
    name: string;
    full_ref: string;
    is_head_pointer: boolean;
    tracked_by: string | null;
    subject: string | null;
}

export interface BlameCommit {
    sha: string;
    short: string;
    author: string;
    author_email: string;
    time: string;
    timestamp: number;
    summary: string;
    uncommitted: boolean;
}

/** Compact per-file blame: unique `commits` + a per-line index into them. */
export interface GitBlame {
    commits: BlameCommit[];
    lines: number[];
}

const fileAtCache = new Map<string, string>();
const fileAtInflight = new Map<string, Promise<string>>();
const FILE_AT_MAX_ENTRIES = 200;
const FILE_AT_MAX_CHARS = 16 * 1024 * 1024;
let fileAtChars = 0;
let fileAtGeneration = 0;
/** A revision that names a commit by hash always reads back the same bytes; HEAD, a branch and the index do not. */
const SHA_RE = /^[0-9a-f]{7,}([~^][0-9]*)*$/i;

function cacheKey(repo: string, rev: string, path: string) {
    return `${repo}\0${rev}\0${path}`;
}

function revisionOf(key: string): string {
    return key.split("\0")[1] ?? "";
}

function rememberFileAt(key: string, content: string): void {
    if (typeof content !== "string") return;
    const previous = fileAtCache.get(key);
    if (previous !== undefined) fileAtChars -= previous.length;
    fileAtCache.delete(key);
    fileAtCache.set(key, content);
    fileAtChars += content.length;
    for (const oldest of fileAtCache.keys()) {
        if (fileAtCache.size <= FILE_AT_MAX_ENTRIES && fileAtChars <= FILE_AT_MAX_CHARS) break;
        fileAtChars -= fileAtCache.get(oldest)!.length;
        fileAtCache.delete(oldest);
    }
}

export function forgetMovingRevisions(repo?: string): void {
    fileAtGeneration += 1;
    const prefix = repo ? `${repo}\0` : null;
    const matches = (key: string) => (!prefix || key.startsWith(prefix)) && !SHA_RE.test(revisionOf(key));
    for (const [key, content] of fileAtCache) {
        if (!matches(key)) continue;
        fileAtCache.delete(key);
        fileAtChars -= content.length;
    }
    for (const key of [...fileAtInflight.keys()]) if (matches(key)) fileAtInflight.delete(key);
}

subscribe("fs-changed", (event) => forgetMovingRevisions(event.repo || undefined));
subscribe("git-refresh", (event) => forgetMovingRevisions(event.repo || undefined));

export const git = {
    status: (repo: string) => invoke<GitStatus>("git_status", { repo }),
    discoverRepos: (root: string) => invoke<DiscoveredRepo[]>("git_discover_repos", { root }),
    overview: (repo: string) => invoke<GitOverview>("git_overview", { repo }),
    diff: (repo: string, path: string, staged: boolean) => invoke<string>("git_diff", { repo, path, staged }),
    stage: (repo: string, path: string) => invoke<void>("git_stage", { repo, path }),
    unstage: (repo: string, path: string) => invoke<void>("git_unstage", { repo, path }),
    stagePaths: (repo: string, paths: string[]) => invoke<void>("git_stage_paths", { repo, paths }),
    unstagePaths: (repo: string, paths: string[]) => invoke<void>("git_unstage_paths", { repo, paths }),
    stageAll: (repo: string) => invoke<void>("git_stage_all", { repo }),
    unstageAll: (repo: string) => invoke<void>("git_unstage_all", { repo }),
    branches: (repo: string) => invoke<GitBranch[]>("git_branches", { repo }),
    worktrees: (repo: string) => invoke<GitWorktree[]>("git_worktree_list", { repo }),
    worktreeCreate: (repo: string, options: CreateGitWorktreeOptions) =>
        invoke<GitWorktree>("git_worktree_create", {
            repo,
            path: options.path,
            branch: options.branch,
            createBranch: options.createBranch,
            startPoint: options.startPoint ?? null,
        }),
    worktreeRemove: (repo: string, path: string, force = false) => invoke<GitWorktree>("git_worktree_remove", { repo, path, force }),
    checkout: (repo: string, branch: string) => invoke<void>("git_checkout", { repo, branch }),
    checkoutSmart: (repo: string, branch: string) => invoke<string>("git_checkout_smart", { repo, branch }),
    branchCreate: (repo: string, name: string, startPoint?: string) =>
        invoke<void>("git_branch_create", {
            repo,
            name,
            startPoint: startPoint ?? null,
        }),
    branchDelete: (repo: string, name: string, force: boolean) => invoke<void>("git_branch_delete", { repo, name, force }),
    branchRename: (repo: string, oldName: string, newName: string) => invoke<void>("git_branch_rename", { repo, oldName, newName }),
    merge: (repo: string, branch: string) => invoke<string>("git_merge", { repo, branch }),
    mergeSquash: (repo: string, branch: string) => invoke<string>("git_merge_squash", { repo, branch }),
    reset: (repo: string, rev: string, mode: "soft" | "mixed" | "hard") => invoke<void>("git_reset", { repo, rev, mode }),
    revert: (repo: string, rev: string) => invoke<void>("git_revert", { repo, rev }),
    log: (repo: string) => invoke<GitCommit[]>("git_log", { repo }),
    show: (repo: string, rev: string) => invoke<string>("git_show", { repo, rev }),
    fileAt: (repo: string, rev: string, path: string): Promise<string> => {
        const key = cacheKey(repo, rev, path);
        const hit = fileAtCache.get(key);
        if (hit !== undefined) {
            fileAtCache.delete(key);
            fileAtCache.set(key, hit);
            return Promise.resolve(hit);
        }
        const pending = fileAtInflight.get(key);
        if (pending) return pending;

        const immutable = SHA_RE.test(rev);
        const generation = fileAtGeneration;
        const request = invoke<string>("git_file_at", { repo, rev, path }).then(
            (content) => {
                if (fileAtInflight.get(key) === request) fileAtInflight.delete(key);
                if (immutable || generation === fileAtGeneration) rememberFileAt(key, content);
                return content;
            },
            (error: unknown) => {
                if (fileAtInflight.get(key) === request) fileAtInflight.delete(key);
                throw error;
            },
        );
        fileAtInflight.set(key, request);
        return request;
    },
    commitFiles: (repo: string, rev: string) => invoke<string[]>("git_commit_files", { repo, rev }),
    blame: (repo: string, path: string, contents?: string | null) => invoke<GitBlame>("git_blame", { repo, path, contents: contents ?? null }),
    commit: (repo: string, message: string) => invoke<string>("git_commit", { repo, message }),
    push: (repo: string) => invoke<string>("git_push", { repo }),
    pull: (repo: string) => invoke<string>("git_pull", { repo }),
    aiCommit: (repo: string, provider: string, model: string) => invoke<string>("git_ai_commit", { repo, provider, model }),
    aiMessage: (repo: string, provider: string, model: string, onChunk: (chunk: string) => void) => {
        const channel = new Channel<string>();
        channel.onmessage = onChunk;
        return invoke<string>("git_ai_message", { repo, provider, model, onChunk: channel });
    },
    prOpen: (repo: string) => invoke<string>("pr_open", { repo }),
    watchStart: (repo: string, token: RepoWatchLeaseToken) => invoke<void>("repo_watch_start", { repo, token }),
    watchStop: (token: RepoWatchLeaseToken) => invoke<void>("repo_watch_stop", { token }),

    discardFile: (repo: string, path: string, mode: DiscardMode) => invoke<void>("git_discard_file", { repo, path, mode }),
    discardFiles: (repo: string, paths: string[], mode: DiscardMode) => invoke<void>("git_discard_files", { repo, paths, mode }),

    stashList: (repo: string) => invoke<GitStash[]>("git_stash_list", { repo }),
    stashPush: (repo: string, mode: StashMode, message?: string | null) =>
        invoke<void>("git_stash_push", {
            repo,
            mode,
            message: message ?? null,
        }),
    stashApply: (repo: string, refname: string, sha: string) => invoke<void>("git_stash_apply", { repo, refname, sha }),
    stashPop: (repo: string, refname: string, sha: string) => invoke<void>("git_stash_pop", { repo, refname, sha }),
    stashDrop: (repo: string, refname: string, sha: string) => invoke<void>("git_stash_drop", { repo, refname, sha }),
    stashBranch: (repo: string, refname: string, sha: string, name: string) => invoke<void>("git_stash_branch", { repo, refname, sha, name }),
    stashRename: (repo: string, refname: string, sha: string, newMessage: string) =>
        invoke<void>("git_stash_rename", { repo, refname, sha, newMessage }),

    remotes: (repo: string) => invoke<GitRemote[]>("git_remotes", { repo }),
    remoteAdd: (repo: string, name: string, url: string) => invoke<void>("git_remote_add", { repo, name, url }),
    remoteRemove: (repo: string, name: string) => invoke<void>("git_remote_remove", { repo, name }),
    remoteRename: (repo: string, oldName: string, newName: string) => invoke<void>("git_remote_rename", { repo, oldName, newName }),
    remoteSetUrl: (repo: string, name: string, url: string) => invoke<void>("git_remote_set_url", { repo, name, url }),
    fetch: (repo: string, remote?: string | null) => invoke<string>("git_fetch", { repo, remote: remote ?? null }),

    remoteBranches: (repo: string, remote: string) => invoke<GitRemoteBranch[]>("git_remote_branches", { repo, remote }),
    checkoutRemoteBranch: (repo: string, remote: string, branch: string, localName?: string | null) =>
        invoke<void>("git_checkout_remote_branch", {
            repo,
            remote,
            branch,
            localName: localName ?? null,
        }),
    deleteRemoteBranch: (repo: string, remote: string, branch: string) => invoke<void>("git_delete_remote_branch", { repo, remote, branch }),
    setUpstream: (repo: string, branch: string, upstream: string | null) => invoke<void>("git_set_upstream", { repo, branch, upstream }),
};

export const isStaged = (f: GitFile) => f.index !== " " && f.index !== "?";
export const hasUnstaged = (f: GitFile) => f.worktree !== " ";
