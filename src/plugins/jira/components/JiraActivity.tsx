import { useState } from "react";
import { formatStamp } from "../../../plugin-api/format";
import { Markdown, Seg } from "../../../plugin-api/ui";
import type { JiraChange, JiraComment, JiraIssueView } from "../api";
import { Avatar } from "./JiraFace";

type Shown = "all" | "comments" | "history";

const KIND_SHOWN: Record<Shown, "comment" | "change" | null> = { all: null, comments: "comment", history: "change" };

/** The issue's comments and edits, newest first, with a box for a new comment on top. */
export function JiraActivity({ issue, busy, onComment }: { issue: JiraIssueView; busy: boolean; onComment: (body: string) => Promise<boolean> }) {
    const [shown, setShown] = useState<Shown>("comments");
    const [draft, setDraft] = useState("");
    const only = KIND_SHOWN[shown];
    const entries = only ? issue.activity.filter((entry) => entry.kind === only) : issue.activity;
    const hiddenComments = issue.commentCount - issue.activity.filter((entry) => entry.kind === "comment").length;

    return (
        <section className="jira-activity" aria-label="Activity">
            <div className="jira-section-title">
                Activity
                <span className="jira-grow" />
                <Seg
                    label="Show in the activity"
                    value={shown}
                    onChange={setShown}
                    options={[
                        { value: "all", label: "All" },
                        { value: "comments", label: "Comments", count: issue.commentCount },
                        { value: "history", label: "History" },
                    ]}
                />
            </div>
            {shown !== "history" && (
                <div className="jira-compose">
                    <textarea value={draft} onChange={(event) => setDraft(event.target.value)} placeholder="Add a comment" rows={3} />
                    <button
                        type="button"
                        className="jira-chip primary"
                        disabled={busy || !draft.trim()}
                        onClick={() => void onComment(draft).then((posted) => posted && setDraft(""))}>
                        Comment
                    </button>
                </div>
            )}
            {entries.length === 0 && <p className="jira-meta">{shown === "history" ? "No changes yet." : "No comments yet."}</p>}
            {entries.map((entry) =>
                entry.kind === "comment" ? (
                    <CommentRow key={`c${entry.comment.id}`} comment={entry.comment} />
                ) : (
                    <ChangeRow key={`h${entry.change.id}`} change={entry.change} />
                ),
            )}
            {shown !== "history" && hiddenComments > 0 && (
                <p className="jira-meta">
                    {hiddenComments} older {hiddenComments === 1 ? "comment" : "comments"} not shown; open the issue in Jira to read them.
                </p>
            )}
        </section>
    );
}

function CommentRow({ comment }: { comment: JiraComment }) {
    return (
        <div className="jira-comment">
            <Avatar person={{ name: comment.author, avatarUrl: comment.authorAvatarUrl }} large />
            <div className="jira-comment-body">
                <div className="jira-comment-head">
                    <b>{comment.author}</b>
                    <span className="jira-meta">{formatStamp(comment.created)}</span>
                </div>
                <Markdown>{comment.body}</Markdown>
            </div>
        </div>
    );
}

function ChangeRow({ change }: { change: JiraChange }) {
    return (
        <div className="jira-comment">
            <Avatar person={{ name: change.author, avatarUrl: change.authorAvatarUrl }} large />
            <div className="jira-comment-body">
                <div className="jira-comment-head">
                    <b>{change.author}</b>
                    <span className="jira-meta">{formatStamp(change.created)}</span>
                </div>
                {change.fields.map((field, index) => (
                    <div key={index} className="jira-change">
                        <span>
                            changed <b>{field.field}</b>
                        </span>
                        {(field.from || field.to) && (
                            <span className="jira-change-values">
                                <span className="jira-change-from">{field.from ?? "None"}</span>
                                <span className="jira-meta">→</span>
                                <span>{field.to ?? "None"}</span>
                            </span>
                        )}
                    </div>
                ))}
            </div>
        </div>
    );
}
