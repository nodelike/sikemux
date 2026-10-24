/*
 * The shapes every code host hands the Git pane come from the plugin API's
 * Rust, in types.generated.ts. What the Git pane sends a host is written here.
 * Statuses and conclusions use GitHub's words, which other hosts map onto.
 */

export type * from "./types.generated";

/** Which repository a call is about. Every read and write carries one. */
export interface RepoRef {
    /** The code host's id, which picks the plugin that answers. */
    provider: string;
    owner: string;
    name: string;
    /** The signed-in account to ask as; with none, the host's default account. */
    account?: string | null;
    /** The server the folder's remote names, so a host with accounts on several servers asks the right one. */
    host?: string;
}

export type RunStatus =
    | "queued"
    | "in_progress"
    | "completed"
    | "requested"
    | "waiting"
    | "pending"
    | "success"
    | "failure"
    | "neutral"
    | "cancelled"
    | "skipped"
    | "timed_out"
    | "action_required";

export interface RunQuery extends RepoRef {
    workflowId?: string;
    branch?: string;
    status?: RunStatus;
    event?: string;
    actor?: string;
    headSha?: string;
    page?: number;
    perPage?: number;
}

/** How far a download has got. `total` is unknown when GitHub did not say. */
export interface DownloadProgress {
    received: number;
    total: number | null;
}

export type MergeMethod = "merge" | "squash" | "rebase";

export type ReviewEvent = "APPROVE" | "REQUEST_CHANGES" | "COMMENT";

export interface NewPull {
    title: string;
    head: string;
    base: string;
    body: string;
    draft: boolean;
}
