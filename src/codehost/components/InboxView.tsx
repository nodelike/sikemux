import { useState } from "react";
import { notify, openUrl, reportError, swallow } from "../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../plugin-api/resources";
import { Checkbox, EmptyState, IconCheck, SkeletonRows, StaleNotice } from "../../plugin-api/ui";
import { type Notification, type RepoRef } from "../api";
import { errorText } from "@shared/lib/format";
import { hostStatusR, inboxR } from "../resources";
import { useAccount, useHost } from "../registry";
import { useBusy } from "./hooks";
import { useNow } from "../../plugin-api/hooks";
import { IssueDetail } from "./IssuesView";
import { PullRight, type PullTab } from "./PullsView";
import { formatAgo } from "@shared/lib/format";
import "../codehost.css";

const REASON: Record<string, string> = {
    review_requested: "Review requested",
    mention: "Mentioned",
    team_mention: "Team mentioned",
    assign: "Assigned",
    author: "You opened it",
    comment: "New comment",
    ci_activity: "CI finished",
    state_change: "State changed",
    subscribed: "Subscribed",
    manual: "Subscribed",
};

export function reasonLabel(reason: string): string {
    return REASON[reason] ?? reason.replace(/_/gu, " ");
}

function Row({ item, now, on, onOpen }: { item: Notification; now: number; on: boolean; onOpen: () => void }) {
    return (
        <button type="button" className="gha-item-row" data-on={on ? "1" : "0"} data-unread={item.unread ? "1" : "0"} onClick={onOpen}>
            <span className="gha-unread-dot" data-on={item.unread ? "1" : "0"} />
            <span className="gha-item-head">
                <span className="gha-item-title">{item.title}</span>
            </span>
            <span className="gha-item-sub">
                <span>{item.repo}</span>
                {item.number !== null && <span className="gha-item-number">#{item.number}</span>}
                <span>{reasonLabel(item.reason)}</span>
            </span>
            <span className="gha-item-when">{formatAgo(item.updatedAt, now)}</span>
        </button>
    );
}

/** A pull request or an issue reads in full, whichever repository it is in; anything else opens on the host. */
export function InboxDetail({ item, active }: { item: Notification; active: boolean }) {
    const host = useHost();
    const account = useAccount();
    const login = useResourceEnabled(active, hostStatusR, host.id, account).data?.login ?? null;
    const [tab, setTab] = useState<PullTab>("conversation");
    const [owner, name] = item.repo.split("/");
    const repo: RepoRef = { provider: host.id, owner, name };
    if (item.number !== null && item.kind === "PullRequest") {
        return (
            <PullRight
                repo={repo}
                project={null}
                cwd={null}
                number={item.number}
                tab={tab}
                commit={null}
                focus={null}
                login={login}
                active={active}
                onTab={setTab}
                onLeaveCommit={() => undefined}
            />
        );
    }
    if (item.number !== null && item.kind === "Issue") {
        return <IssueDetail repo={repo} number={item.number} active={active} />;
    }
    return (
        <EmptyState
            title={item.title}
            message={`${item.repo} · ${reasonLabel(item.reason)}`}
            action={
                item.url
                    ? { label: `Open on ${host.name}`, onClick: () => void openUrl(item.url ?? "").catch(swallow(`open ${host.name}`)) }
                    : undefined
            }
        />
    );
}

interface Props {
    /** The notification open beside the list. */
    shownId: string | null;
    onOpen: (item: Notification) => void;
    active: boolean;
}

/** The signed-in account's notifications, newest first; opening an unread one marks it read. */
export function InboxList({ shownId, onOpen, active }: Props) {
    const host = useHost();
    const [all, setAll] = useState(false);
    const account = useAccount();
    const inbox = useResourceEnabled(active, inboxR, host.id, account, all);
    const now = useNow(false);
    const [busy, runBusy] = useBusy();

    const rows = inbox.data ?? [];
    const unread = rows.filter((row) => row.unread).length;

    const refresh = () => invalidate((kind) => kind === "host.inbox");
    const open = (item: Notification) => {
        onOpen(item);
        if (!item.unread) return;
        void host.api.markRead(account, item.id).then(refresh).catch(swallow("mark it read"));
    };
    const readEverything = () =>
        runBusy(() =>
            host.api
                .markAllRead(account)
                .then(() => {
                    notify("success", "Inbox cleared");
                    refresh();
                })
                .catch(reportError("Could not clear the inbox")),
        );

    if (inbox.status === "loading" && !inbox.data) return <SkeletonRows rows={8} label="Loading notifications" />;
    if (inbox.error && !inbox.data) {
        return (
            <EmptyState
                title="Could not read notifications"
                message={errorText(inbox.error)}
                tone="error"
                action={{ label: "Try again", onClick: () => void inbox.refresh() }}
            />
        );
    }
    return (
        <div className="gha-list pr-list">
            <StaleNotice handle={inbox} />
            <div className="gha-list-head">
                <Checkbox checked={all} onChange={setAll}>
                    Include read
                </Checkbox>
                <span className="gha-dim">
                    {unread} unread
                    {unread > 0 && (
                        <button type="button" className="gha-link" disabled={busy} onClick={readEverything}>
                            Mark all read
                        </button>
                    )}
                </span>
            </div>
            {rows.length === 0 ? (
                <EmptyState icon={<IconCheck size={20} />} title="Nothing waiting" message="No notifications." />
            ) : (
                rows.map((item) => <Row key={item.id} item={item} now={now} on={item.id === shownId} onOpen={() => open(item)} />)
            )}
        </div>
    );
}
