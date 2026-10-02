import { useState, type ReactNode } from "react";
import { copyText, notify, reportError, swallow } from "../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../plugin-api/resources";
import { IconCheck, IconClose, IconCommit, IconEye, IconGit, IconMerge, IconPencil, IconPush, IconUser } from "../../plugin-api/ui";
import { hostApi, type RepoRef, type ReviewEvent, type TimelineItem } from "../api";
import { useHost, usePictureOf } from "../registry";
import { timelineR } from "../resources";
import { formatAgo } from "../runStatus";
import { SectionIcon } from "./ActionsIcon";
import { Avatar, Initial, Prose } from "./Pictures";

const REVIEW_WORD: Record<string, string> = {
    APPROVED: "approved these changes",
    CHANGES_REQUESTED: "requested changes",
    COMMENTED: "reviewed",
    DISMISSED: "had a review dismissed",
};

const REVIEW_DONE: Record<ReviewEvent, string> = {
    APPROVE: "Approved",
    REQUEST_CHANGES: "Asked for changes on",
    COMMENT: "Reviewed",
};

const ROLE: Record<string, string> = {
    OWNER: "Owner",
    MEMBER: "Member",
    COLLABORATOR: "Collaborator",
    CONTRIBUTOR: "Contributor",
    FIRST_TIME_CONTRIBUTOR: "First-time contributor",
    FIRST_TIMER: "First-time contributor",
};

/** What someone wrote at a moment in the thread, which the opening description is too. */
export interface Post {
    key: string;
    author: string | null;
    avatarUrl: string | null;
    association: string | null;
    at: string | null;
    body: string;
    review: string | null;
}

export function Face({ login, url }: { login: string | null; url: string | null }) {
    const picture = usePictureOf(login, url);
    return picture ? <Avatar url={picture} login={login} /> : <Initial login={login} />;
}

function Name({ login }: { login: string | null }) {
    return <span className="gha-comment-author">{login ?? "someone"}</span>;
}

/** Someone named in the timeline, with their face beside their name. */
function Actor({ login, url }: { login: string | null; url: string | null }) {
    return (
        <span className="gha-tl-actor">
            <Face login={login} url={url} />
            <Name login={login} />
        </span>
    );
}

function PostCard({ post, now }: { post: Post; now: number }) {
    const role = post.association ? ROLE[post.association] : undefined;
    return (
        <div className="gha-tl-post">
            <div className="gha-comment" data-review={post.review ?? undefined}>
                <div className="gha-comment-head">
                    <Face login={post.author} url={post.avatarUrl} />
                    <Name login={post.author} />
                    {post.review ? (
                        <span className="gha-review-state" data-state={post.review}>
                            {REVIEW_WORD[post.review] ?? post.review.toLowerCase()}
                        </span>
                    ) : (
                        <span className="gha-dim">commented</span>
                    )}
                    <span className="gha-dim">{formatAgo(post.at, now)}</span>
                    <span className="gha-page-spacer" />
                    {role && <span className="gha-role">{role}</span>}
                </div>
                {post.body.trim() ? <Prose>{post.body}</Prose> : <div className="gha-comment-empty">No description provided.</div>}
            </div>
        </div>
    );
}

function Event({ icon, tone, children }: { icon: ReactNode; tone?: string; children: ReactNode }) {
    return (
        <div className="gha-tl-event">
            <span className="gha-tl-badge" data-tone={tone}>
                {icon}
            </span>
            <span className="gha-tl-text">{children}</span>
        </div>
    );
}

function Commits({ commits, now }: { commits: readonly TimelineItem[]; now: number }) {
    const authors = [...new Set(commits.map((commit) => commit.actor))];
    const first = commits.find((commit) => commit.actor === authors[0]);
    return (
        <div className="gha-tl-commits">
            <Event icon={<IconPush size={12} />}>
                <Actor login={authors[0] ?? null} url={first?.avatarUrl ?? null} />
                {authors.length > 1 && <span className="gha-dim">and others</span>}
                <span className="gha-dim">
                    added {commits.length} commit{commits.length === 1 ? "" : "s"} {formatAgo(commits[0]?.at ?? null, now)}
                </span>
            </Event>
            {commits.map((commit) => (
                <div className="gha-tl-commit" key={commit.sha ?? commit.message}>
                    <span className="gha-tl-commit-dot">
                        <IconCommit size={12} />
                    </span>
                    <span className="gha-tl-commit-message">{(commit.message ?? "").split("\n")[0]}</span>
                    {commit.sha && (
                        <button
                            type="button"
                            className="gha-tl-sha"
                            title="Copy the full commit"
                            onClick={() =>
                                void copyText(commit.sha ?? "")
                                    .then(() => notify("success", `Copied ${commit.sha?.slice(0, 7)}`))
                                    .catch(swallow("copy the commit"))
                            }>
                            {commit.sha.slice(0, 7)}
                        </button>
                    )}
                </div>
            ))}
        </div>
    );
}

type Block = { kind: "item"; item: TimelineItem } | { kind: "commits"; commits: TimelineItem[] };

/** Commits pushed together read as one group, the way GitHub shows them. */
function blocksOf(items: readonly TimelineItem[]): Block[] {
    const blocks: Block[] = [];
    for (const item of items) {
        const last = blocks.at(-1);
        if (item.kind === "committed" && last?.kind === "commits") last.commits.push(item);
        else if (item.kind === "committed") blocks.push({ kind: "commits", commits: [item] });
        else blocks.push({ kind: "item", item });
    }
    return blocks;
}

function ItemView({ item, now, base }: { item: TimelineItem; now: number; base: string | null }) {
    const when = <span className="gha-dim">{formatAgo(item.at, now)}</span>;
    const who = <Actor login={item.actor} url={item.avatarUrl} />;
    const short = item.sha?.slice(0, 7);
    switch (item.kind) {
        case "commented":
            return (
                <PostCard
                    now={now}
                    post={{
                        key: "",
                        author: item.actor,
                        avatarUrl: item.avatarUrl,
                        association: item.association,
                        at: item.at,
                        body: item.body ?? "",
                        review: null,
                    }}
                />
            );
        case "reviewed": {
            const verdict = (item.state ?? "commented").toUpperCase();
            if ((item.body ?? "").trim()) {
                return (
                    <PostCard
                        now={now}
                        post={{
                            key: "",
                            author: item.actor,
                            avatarUrl: item.avatarUrl,
                            association: item.association,
                            at: item.at,
                            body: item.body ?? "",
                            review: verdict,
                        }}
                    />
                );
            }
            const tone = verdict === "APPROVED" ? "live" : verdict === "CHANGES_REQUESTED" ? "danger" : undefined;
            return (
                <Event icon={verdict === "APPROVED" ? <IconCheck size={12} /> : <IconEye size={12} />} tone={tone}>
                    {who}
                    <span className="gha-review-state" data-state={verdict}>
                        {REVIEW_WORD[verdict] ?? "reviewed"}
                    </span>
                    {when}
                </Event>
            );
        }
        case "review_requested":
            return (
                <Event icon={<IconEye size={12} />}>
                    {who} <span className="gha-dim">requested a review from</span> <Actor login={item.subject} url={null} /> {when}
                </Event>
            );
        case "review_request_removed":
            return (
                <Event icon={<IconEye size={12} />}>
                    {who} <span className="gha-dim">removed the review request for</span> <Actor login={item.subject} url={null} /> {when}
                </Event>
            );
        case "head_ref_force_pushed":
            return (
                <Event icon={<IconPush size={12} />}>
                    {who} <span className="gha-dim">force-pushed the branch{short ? ` to ${short}` : ""}</span> {when}
                </Event>
            );
        case "merged":
            return (
                <Event icon={<IconMerge size={12} />} tone="acc">
                    {who}{" "}
                    <span className="gha-dim">
                        merged {short ? `commit ${short} ` : ""}into {base ?? "the base branch"}
                    </span>{" "}
                    {when}
                </Event>
            );
        case "closed":
            return (
                <Event icon={<IconClose size={12} />} tone={item.state === "not_planned" ? undefined : "acc"}>
                    {who} <span className="gha-dim">closed this{item.state === "not_planned" ? " as not planned" : ""}</span> {when}
                </Event>
            );
        case "reopened":
            return (
                <Event icon={<SectionIcon section="issues" size={12} />} tone="live">
                    {who} <span className="gha-dim">reopened this</span> {when}
                </Event>
            );
        case "labeled":
        case "unlabeled":
            return (
                <Event icon={<SectionIcon section="releases" size={12} />}>
                    {who} <span className="gha-dim">{item.kind === "labeled" ? "added" : "removed"} the</span>{" "}
                    <span className="gha-label">{item.subject}</span> <span className="gha-dim">label</span> {when}
                </Event>
            );
        case "assigned":
        case "unassigned":
            return (
                <Event icon={<IconUser size={12} />}>
                    {who} <span className="gha-dim">{item.kind === "assigned" ? "assigned" : "unassigned"}</span>{" "}
                    <Actor login={item.subject} url={null} /> {when}
                </Event>
            );
        case "renamed":
            return (
                <Event icon={<IconPencil size={12} />}>
                    {who} <span className="gha-dim">changed the title to</span> <span className="gha-tl-quote">{item.subject}</span> {when}
                </Event>
            );
        case "ready_for_review":
            return (
                <Event icon={<IconEye size={12} />}>
                    {who} <span className="gha-dim">marked this ready for review</span> {when}
                </Event>
            );
        case "convert_to_draft":
            return (
                <Event icon={<IconPencil size={12} />}>
                    {who} <span className="gha-dim">marked this as a draft</span> {when}
                </Event>
            );
        case "head_ref_deleted":
            return (
                <Event icon={<IconGit size={12} />}>
                    {who} <span className="gha-dim">deleted the branch</span> {when}
                </Event>
            );
        case "head_ref_restored":
            return (
                <Event icon={<IconGit size={12} />}>
                    {who} <span className="gha-dim">restored the branch</span> {when}
                </Event>
            );
        case "referenced":
            return (
                <Event icon={<IconCommit size={12} />}>
                    {who} <span className="gha-dim">referenced this{short ? ` in commit ${short}` : ""}</span> {when}
                </Event>
            );
        case "cross-referenced":
            return (
                <Event icon={<SectionIcon section="issues" size={12} />}>
                    {who} <span className="gha-dim">mentioned this in</span> <span className="gha-tl-quote">{item.subject}</span> {when}
                </Event>
            );
        default:
            return null;
    }
}

interface Props {
    repo: RepoRef;
    number: number;
    active: boolean;
    now: number;
    opening?: Post | null;
    /** The branch a merge went into, which the timeline's merge event leaves out. */
    base?: string | null;
    /** Offers approving and asking for changes, which hosts refuse on your own pull request. */
    review?: { mine: boolean } | null;
    /** Sits between the timeline and the composer, where GitHub puts its merge box. */
    children?: ReactNode;
    /** Sits beside the composer's own buttons, such as closing the issue. */
    extraActions?: ReactNode;
    /** Leaves pushed commits out, for a pull request whose commits are listed beside the thread. */
    withoutCommits?: boolean;
}

export function CommentThread({
    repo,
    number,
    active,
    now,
    opening = null,
    base = null,
    review = null,
    children,
    extraActions,
    withoutCommits = false,
}: Props) {
    const timeline = useResourceEnabled(active, timelineR, repo, number);
    const host = useHost();
    const [draft, setDraft] = useState("");
    const [busy, setBusy] = useState<ReviewEvent | "comment" | null>(null);
    const blocks = blocksOf(timeline.data ?? []).filter((block) => !withoutCommits || block.kind !== "commits");
    const written = draft.trim().length > 0;
    const refreshThread = (also?: string) => invalidate((kind) => kind === "host.timeline" || kind === "host.comments" || kind === also);

    const comment = async () => {
        setBusy("comment");
        try {
            await hostApi(repo.provider).addComment(repo, number, draft.trim());
            setDraft("");
            refreshThread();
            notify("success", "Comment added");
        } catch (error) {
            reportError("Could not add the comment")(error);
        } finally {
            setBusy(null);
        }
    };

    const send = async (event: ReviewEvent) => {
        setBusy(event);
        try {
            await hostApi(repo.provider).reviewPull(repo, number, event, draft.trim());
            setDraft("");
            notify("success", `${REVIEW_DONE[event]} #${number}`);
            refreshThread("host.pullReviews");
            invalidate((kind) => kind === "host.pull");
        } catch (error) {
            reportError("Could not send the review")(error);
        } finally {
            setBusy(null);
        }
    };

    return (
        <section className="gha-thread">
            <div className="gha-timeline">
                {opening && <PostCard post={opening} now={now} />}
                {blocks.map((block, index) =>
                    block.kind === "commits" ? (
                        <Commits key={`commits-${block.commits[0]?.sha ?? index}`} commits={block.commits} now={now} />
                    ) : (
                        <ItemView key={`${block.item.kind}-${block.item.id ?? block.item.sha ?? index}`} item={block.item} now={now} base={base} />
                    ),
                )}
            </div>
            {children && <div className="gha-tl-tail">{children}</div>}
            <div className="gha-tl-tail">
                <div className="gha-composer">
                    <textarea
                        className="gha-composer-box"
                        value={draft}
                        onChange={(event) => setDraft(event.target.value)}
                        placeholder={review ? "Leave a comment or a review" : "Leave a comment"}
                        rows={3}
                    />
                    <div className="gha-composer-foot">
                        {review?.mine && <span className="gha-dim">{host.name} does not let you approve your own pull request.</span>}
                        <span className="gha-page-spacer" />
                        {extraActions}
                        {review && !review.mine && host.capabilities.pulls.requestChanges && (
                            <button
                                type="button"
                                className="gha-btn"
                                disabled={busy !== null || !written}
                                onClick={() => void send("REQUEST_CHANGES")}>
                                Request changes
                            </button>
                        )}
                        {review && !review.mine && (
                            <button type="button" className="gha-btn" disabled={busy !== null} onClick={() => void send("APPROVE")}>
                                {busy === "APPROVE" ? "Approving…" : "Approve"}
                            </button>
                        )}
                        <button type="button" className="gha-btn primary" disabled={busy !== null || !written} onClick={() => void comment()}>
                            {busy === "comment" ? "Sending…" : "Comment"}
                        </button>
                    </div>
                </div>
            </div>
        </section>
    );
}
