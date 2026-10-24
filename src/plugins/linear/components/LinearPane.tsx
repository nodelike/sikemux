import { useState } from "react";
import { copyWithNotice, notify, openUrl, reportError, swallow } from "../../../plugin-api/host";
import { errorText, formatStamp } from "../../../plugin-api/format";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { Dropdown, EmptyState, Loaded, Markdown, Pending, SendToAgentMenu, SignedIn, StaleNotice } from "../../../plugin-api/ui";
import { linearApi, type LinearChange, type LinearStatus } from "../api";
import { linear, linearIssueV, linearListV } from "../resources";
import { updateLinearView, useLinearView } from "../state";
import type { LinearIssueRow } from "../types.generated";
import { LinearSignIn } from "./LinearSignIn";
import "../linear.css";

export function LinearPane({ paneId, active }: { paneId: string; active: boolean }) {
    return (
        <SignedIn service={linear} active={active} signIn={(status) => <LinearSignIn status={status} onSignedIn={() => linear.refresh()} />}>
            {(status) => <LinearWorkspace paneId={paneId} active={active} status={status} />}
        </SignedIn>
    );
}

function StateDot({ color }: { color: string }) {
    return <span className="linear-state-dot" style={{ background: color }} aria-hidden="true" />;
}

function LinearWorkspace({ paneId, active, status }: { paneId: string; active: boolean; status: LinearStatus }) {
    const view = useLinearView(paneId);
    const workspace =
        status.workspaces.find((each) => each.id === view.workspace) ?? status.workspaces.find((each) => each.isDefault) ?? status.workspaces[0];
    const [words, setWords] = useState(view.query);
    const issues = useResourceEnabled(active && !!workspace, linearListV, paneId, workspace?.id ?? "", view.query);
    if (!workspace) return null;

    const row = (issue: LinearIssueRow) => (
        <button
            key={issue.id}
            type="button"
            className={`linear-row${issue.identifier === view.issue ? " active" : ""}`}
            onClick={() => updateLinearView(paneId, { issue: issue.identifier })}>
            <StateDot color={issue.state.color} />
            <span className="linear-row-id">{issue.identifier}</span>
            <span className="linear-row-title">{issue.title}</span>
            <span className="linear-row-state">{issue.state.name}</span>
        </button>
    );

    return (
        <div className={`linear-pane${view.issue ? " with-issue" : ""}`}>
            <nav className="linear-list" aria-label="Linear issues">
                <div className="linear-list-head">
                    {status.workspaces.length > 1 ? (
                        <Dropdown
                            label="Workspace"
                            value={workspace.id}
                            options={status.workspaces.map((each) => ({ value: each.id, label: each.name, detail: each.urlKey }))}
                            onChange={(id) => updateLinearView(paneId, { workspace: id, issue: null })}
                        />
                    ) : (
                        <span className="linear-workspace">{workspace.name}</span>
                    )}
                    <button type="button" className="plugin-chip" onClick={() => void issues.refresh()}>
                        Refresh
                    </button>
                </div>
                <input
                    className="plugin-input"
                    value={words}
                    onChange={(event) => setWords(event.target.value)}
                    onKeyDown={(event) => {
                        if (event.key === "Enter") updateLinearView(paneId, { query: words.trim() });
                        if (event.key === "Escape") {
                            setWords("");
                            updateLinearView(paneId, { query: "" });
                        }
                    }}
                    placeholder="Search issues, or an id such as ENG-123"
                    aria-label="Search Linear issues"
                    spellCheck={false}
                />
                {issues.data && <div className="plugin-heading">{issues.data.heading}</div>}
                <StaleNotice handle={issues} />
                <Loaded handle={issues} loading="Loading issues" empty={(found) => (found.issues.length > 0 ? null : found.empty)}>
                    {(found) => <div className="linear-rows">{found.issues.map(row)}</div>}
                </Loaded>
                <button
                    type="button"
                    className="plugin-signout"
                    onClick={() =>
                        void linearApi
                            .signOut(workspace.id)
                            .then(() => linear.refresh())
                            .catch(reportError("sign out of Linear"))
                    }>
                    Sign out of {workspace.name}
                </button>
            </nav>
            {view.issue ? (
                <IssueView
                    paneId={paneId}
                    active={active}
                    workspace={workspace.id}
                    id={view.issue}
                    onClose={() => updateLinearView(paneId, { issue: null })}
                />
            ) : (
                <div className="linear-detail">
                    <EmptyState message="Pick an issue to read it, comment on it or move it." />
                </div>
            )}
        </div>
    );
}

function IssueView({
    paneId,
    active,
    workspace,
    id,
    onClose,
}: {
    paneId: string;
    active: boolean;
    workspace: string;
    id: string;
    onClose: () => void;
}) {
    const found = useResourceEnabled(active, linearIssueV, `${paneId}/issue`, workspace, id);
    const [draft, setDraft] = useState("");
    const [busy, setBusy] = useState(false);
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
    const issue = found.data;

    const change = (what: LinearChange, done: string) => {
        setBusy(true);
        void linearApi
            .update(workspace, id, what)
            .then(() => notify("success", done))
            .catch((error: unknown) => notify("error", `Change ${id}: ${errorText(error)}`))
            .finally(() => setBusy(false));
    };
    const comment = () => {
        const body = draft.trim();
        if (!body || busy) return;
        setBusy(true);
        void linearApi
            .comment(workspace, id, body)
            .then(() => setDraft(""))
            .catch(reportError(`Comment on ${id}`))
            .finally(() => setBusy(false));
    };

    if (!issue) {
        return (
            <section className="linear-detail">
                <Pending handle={found} loading={`Loading ${id}`} />
            </section>
        );
    }

    return (
        <section className="linear-detail" aria-label={`${issue.identifier} ${issue.title}`}>
            <StaleNotice handle={found} />
            <header className="linear-detail-head">
                <span className="linear-row-id">{issue.identifier}</span>
                <span className="linear-detail-actions">
                    <button type="button" className="plugin-chip" onClick={() => void openUrl(issue.url).catch(swallow("open the issue"))}>
                        Open in Linear
                    </button>
                    <button type="button" className="plugin-chip" onClick={() => copyWithNotice(issue.branchName, "the branch name")}>
                        Copy branch
                    </button>
                    <button type="button" className="plugin-chip" onClick={(event) => setMenu({ x: event.clientX, y: event.clientY })}>
                        Send to agent
                    </button>
                    <button type="button" className="plugin-chip" aria-label="Close the issue" onClick={onClose}>
                        ✕
                    </button>
                </span>
            </header>
            <h2 className="linear-title">{issue.title}</h2>
            <div className="linear-facts">
                <Dropdown
                    label="State"
                    value={issue.state.id}
                    options={issue.states.map((state) => ({ value: state.id, label: state.name }))}
                    onChange={(stateId) => {
                        const state = issue.states.find((each) => each.id === stateId);
                        if (state && state.id !== issue.state.id) change({ state: state.name }, `${issue.identifier} is ${state.name}`);
                    }}
                />
                <span className="linear-fact">{issue.assignee ?? "Unassigned"}</span>
                {!issue.assignee && (
                    <button
                        type="button"
                        className="plugin-chip"
                        disabled={busy}
                        onClick={() => change({ assignee: "me" }, `${issue.identifier} is yours`)}>
                        Assign to me
                    </button>
                )}
                {issue.priority && <span className="linear-fact">{issue.priority}</span>}
                {issue.labels.map((label) => (
                    <span key={label.name} className="linear-label">
                        <StateDot color={label.color} />
                        {label.name}
                    </span>
                ))}
            </div>
            <div className="linear-body">
                {issue.description ? (
                    <Markdown className="prose linear-description">{issue.description}</Markdown>
                ) : (
                    <p className="plugin-note">No description.</p>
                )}
                <h3 className="plugin-heading">Comments</h3>
                {issue.comments.length === 0 && <p className="plugin-note">No comments yet.</p>}
                {issue.comments.map((each) => (
                    <article key={each.id} className="linear-comment">
                        <header>
                            <b>{each.author}</b>
                            <span>{formatStamp(each.createdAt)}</span>
                        </header>
                        <Markdown className="prose">{each.body}</Markdown>
                    </article>
                ))}
            </div>
            <div className="linear-composer">
                <textarea
                    value={draft}
                    onChange={(event) => setDraft(event.target.value)}
                    onKeyDown={(event) => {
                        if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) comment();
                    }}
                    placeholder="Comment (⌘↵ to send)"
                    aria-label={`Comment on ${issue.identifier}`}
                />
                <button type="button" className="plugin-chip primary" disabled={!draft.trim() || busy} onClick={comment}>
                    {busy ? "Saving…" : "Comment"}
                </button>
            </div>
            {menu && <SendToAgentMenu x={menu.x} y={menu.y} alignRight delivery={() => ({ text: issue.forAgent })} onClose={() => setMenu(null)} />}
        </section>
    );
}
