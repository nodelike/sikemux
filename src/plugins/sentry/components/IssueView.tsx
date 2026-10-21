import { useState } from "react";
import { copyWithNotice, notify, openUrl, promptDialog, swallow } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { Dropdown, SendToAgentMenu, Seg, SkeletonRows, StaleNotice } from "../../../plugin-api/ui";
import { errorText } from "../../../plugin-api/format";
import { sentryApi, type Breadcrumb, type IssueAction, type IssueDetail, type Member } from "../api";
import { ago, count, levelTone } from "../format";
import { sentry, sentryIssueR, sentryMembersR } from "../resources";
import { LevelMark, Substatus } from "./parts";
import { StackTrace } from "./StackTrace";

type IssueTab = "stack" | "breadcrumbs" | "tags";

const UNASSIGNED = "";

export function IssueView({ org, id, active, onBack }: { org: string; id: string; active: boolean; onBack: () => void }) {
    const [tab, setTab] = useState<IssueTab>("stack");
    const [busy, setBusy] = useState(false);
    const [menu, setMenu] = useState<{ x: number; y: number; text: string } | null>(null);
    const issue = useResourceEnabled(active, sentryIssueR, org, id);
    const members = useResourceEnabled(active, sentryMembersR, org);
    const detail = issue.data;

    const act = async (work: () => Promise<unknown>, done: string) => {
        setBusy(true);
        try {
            await work();
            notify("success", done);
            sentry.refresh();
        } catch (failure) {
            notify("error", errorText(failure));
        } finally {
            setBusy(false);
        }
    };
    const change = (action: IssueAction, done: string) => void act(() => sentryApi.change(id, { action }), done);
    const assign = (actor: string) =>
        void act(() => sentryApi.change(id, { assign: actor }), actor ? `Assigned to ${nameOf(members.data ?? [], actor)}` : "Unassigned");
    const comment = async () => {
        const text = await promptDialog({ title: `Comment on ${detail?.shortId ?? "the issue"}`, label: "Comment", confirmLabel: "Comment" });
        if (text?.trim()) void act(() => sentryApi.comment(id, text.trim()), "Comment added");
    };
    const askAgent = async (anchor: DOMRect) => {
        try {
            const text = await sentryApi.brief(id);
            setMenu({ x: anchor.right, y: anchor.bottom + 4, text });
        } catch (failure) {
            notify("error", errorText(failure));
        }
    };

    return (
        <section className="sentry-detail" aria-label={detail ? `Issue ${detail.shortId}` : "Issue"}>
            <header className="sentry-detail-head">
                <button type="button" className="sentry-back" onClick={onBack} aria-label="Back to issues">
                    ‹ Issues
                </button>
                {detail ? (
                    <div className="sentry-detail-actions">
                        {detail.status === "unresolved" ? (
                            <>
                                <button type="button" className="sentry-button primary" disabled={busy} onClick={() => change("resolve", "Resolved")}>
                                    Resolve
                                </button>
                                <button
                                    type="button"
                                    className="sentry-button"
                                    disabled={busy}
                                    title="Resolve once a newer release ships"
                                    onClick={() => change("resolveInNextRelease", "Resolves in the next release")}>
                                    In next release
                                </button>
                                <button
                                    type="button"
                                    className="sentry-button"
                                    disabled={busy}
                                    title="Hide it until it happens more"
                                    onClick={() => change("ignore", "Ignored until it escalates")}>
                                    Ignore
                                </button>
                            </>
                        ) : (
                            <button type="button" className="sentry-button" disabled={busy} onClick={() => change("unresolve", "Reopened")}>
                                Reopen
                            </button>
                        )}
                        <Dropdown
                            className="sentry-assign"
                            label="Assignee"
                            value={detail.assignee ? `${detail.assignee.kind}:${detail.assignee.id}` : UNASSIGNED}
                            options={[
                                { value: UNASSIGNED, label: "Unassigned" },
                                ...(detail.assignee &&
                                !(members.data ?? []).some((each) => each.actor === `${detail.assignee?.kind}:${detail.assignee?.id}`)
                                    ? [{ value: `${detail.assignee.kind}:${detail.assignee.id}`, label: detail.assignee.name }]
                                    : []),
                                ...(members.data ?? []).map((member) => ({
                                    value: member.actor,
                                    label: member.isMe ? `${member.name} (me)` : member.name,
                                    detail: member.email ?? undefined,
                                })),
                            ]}
                            onChange={assign}
                        />
                        <button
                            type="button"
                            className="sentry-button"
                            onClick={(event) => void askAgent(event.currentTarget.getBoundingClientRect())}>
                            Ask an agent
                        </button>
                        <button type="button" className="sentry-button" onClick={() => void comment()}>
                            Comment
                        </button>
                        <button type="button" className="sentry-button" onClick={() => void openUrl(detail.link).catch(swallow("open Sentry"))}>
                            Open in Sentry
                        </button>
                    </div>
                ) : null}
            </header>
            {menu ? <SendToAgentMenu x={menu.x} y={menu.y} alignRight delivery={() => ({ text: menu.text })} onClose={() => setMenu(null)} /> : null}
            {issue.status === "error" && !detail ? (
                <p className="sentry-note sentry-error">{issue.error}</p>
            ) : !detail ? (
                <SkeletonRows rows={8} label="Reading the issue" />
            ) : (
                <>
                    <StaleNotice handle={issue} />
                    <Summary detail={detail} />
                    <Seg
                        label="Issue view"
                        value={tab}
                        onChange={setTab}
                        options={[
                            { value: "stack", label: "Stack trace" },
                            { value: "breadcrumbs", label: "Breadcrumbs", count: detail.event?.breadcrumbs.length ?? 0 },
                            { value: "tags", label: "Tags", count: detail.event?.tags.length ?? 0 },
                        ]}
                    />
                    <div className="sentry-tab">
                        {!detail.event ? (
                            <p className="sentry-note">Sentry kept no event for this issue.</p>
                        ) : tab === "stack" ? (
                            <StackTrace exceptions={detail.event.exceptions} message={detail.event.message} />
                        ) : tab === "breadcrumbs" ? (
                            <Breadcrumbs crumbs={detail.event.breadcrumbs} />
                        ) : (
                            <Tags detail={detail} />
                        )}
                    </div>
                </>
            )}
        </section>
    );
}

function nameOf(members: Member[], actor: string): string {
    return members.find((member) => member.actor === actor)?.name ?? actor;
}

function Summary({ detail }: { detail: IssueDetail }) {
    return (
        <div className="sentry-summary">
            <div className="sentry-summary-title">
                <LevelMark tone={levelTone(detail.level)} label={detail.level} />
                <h2>
                    {detail.kind ? <strong>{detail.kind}</strong> : null}
                    <span>{detail.kind && detail.message ? detail.message : detail.title}</span>
                </h2>
            </div>
            <div className="sentry-issue-meta">
                <button
                    type="button"
                    className="sentry-link sentry-mono"
                    title="Copy the short id"
                    onClick={() => copyWithNotice(detail.shortId, "Short id")}>
                    {detail.shortId}
                </button>
                <span className={`sentry-chip status-${detail.status}`}>{detail.status}</span>
                <Substatus value={detail.substatus} />
                {detail.unhandled ? <span className="sentry-chip unhandled">Unhandled</span> : null}
                <span className="sentry-culprit">{detail.culprit}</span>
            </div>
            <dl className="sentry-facts">
                <Fact term="Events" value={count(detail.events)} />
                <Fact term="Users" value={count(detail.users)} />
                <Fact term="First seen" value={`${ago(detail.firstSeen)} ago${detail.firstRelease ? ` in ${detail.firstRelease}` : ""}`} />
                <Fact term="Last seen" value={`${ago(detail.lastSeen)} ago${detail.lastRelease ? ` in ${detail.lastRelease}` : ""}`} />
                <Fact term="Environment" value={detail.event?.environment ?? "—"} />
                <Fact term="Assignee" value={detail.assignee?.name ?? "Unassigned"} />
                {detail.comments > 0 ? <Fact term="Comments" value={String(detail.comments)} /> : null}
            </dl>
            {detail.event?.request ? <p className="sentry-request sentry-mono">{detail.event.request}</p> : null}
        </div>
    );
}

function Fact({ term, value }: { term: string; value: string }) {
    return (
        <div>
            <dt>{term}</dt>
            <dd>{value}</dd>
        </div>
    );
}

export function crumbTime(at: string | null): string {
    return at ? at.slice(11, 19) : "";
}

function Breadcrumbs({ crumbs }: { crumbs: Breadcrumb[] }) {
    if (crumbs.length === 0) return <p className="sentry-note">The SDK sent no breadcrumbs.</p>;
    return (
        <ol className="sentry-crumbs" aria-label="Breadcrumbs">
            {[...crumbs].reverse().map((crumb, index) => (
                <li
                    key={`${crumb.at}-${index}`}
                    className={crumb.level === "error" || crumb.level === "fatal" ? "error" : crumb.level === "warning" ? "warn" : undefined}>
                    <span className="sentry-crumb-category">{crumb.category}</span>
                    <span className="sentry-crumb-message">{crumb.message}</span>
                    <span className="sentry-muted sentry-mono">{crumbTime(crumb.at)}</span>
                </li>
            ))}
        </ol>
    );
}

function Tags({ detail }: { detail: IssueDetail }) {
    const event = detail.event;
    if (!event) return null;
    return (
        <div className="sentry-tags">
            {event.contexts.length > 0 ? (
                <dl className="sentry-facts">
                    {event.contexts.map(([key, value]) => (
                        <Fact key={key} term={key} value={value} />
                    ))}
                </dl>
            ) : null}
            <ul className="sentry-tag-list" aria-label="Tags">
                {event.tags.map(([key, value]) => (
                    <li key={key}>
                        <span className="sentry-muted">{key}</span>
                        <span className="sentry-mono">{value}</span>
                    </li>
                ))}
            </ul>
        </div>
    );
}
