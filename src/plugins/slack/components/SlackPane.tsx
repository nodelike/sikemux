import { useState } from "react";
import { copyText, notify, openUrl, reportError, swallow } from "../../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../../plugin-api/resources";
import { Dropdown, EmptyState, Markdown, SendToAgentMenu, SkeletonRows } from "../../../plugin-api/ui";
import { failureMessage, refreshSlack, slackApi, type SlackChannel, type SlackMessage, type SlackStatus } from "../api";
import { slackChannelsR, slackHistoryR, slackStatusR, slackThreadR } from "../resources";
import { threadForAgent, updateSlackView, useSlackView } from "../state";
import { SlackSignIn } from "./SlackSignIn";
import "../slack.css";

export function SlackPane({ paneId, active }: { paneId: string; active: boolean }) {
    const status = useResourceEnabled(active, slackStatusR);
    if (status.data === undefined && status.status !== "error") return <SkeletonRows rows={5} label="Connecting to Slack" />;
    if (!status.data?.configured || status.data.authFailed) return <SlackSignIn status={status.data} onSignedIn={() => refreshSlack()} />;
    return <SlackWorkspace paneId={paneId} active={active} status={status.data} />;
}

function when(at: string): string {
    const time = Date.parse(at);
    if (Number.isNaN(time)) return "";
    const date = new Date(time);
    const today = new Date().toDateString() === date.toDateString();
    return today
        ? date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })
        : date.toLocaleString(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
}

const KIND_MARK: Record<SlackChannel["kind"], string> = { channel: "#", private: "🔒", dm: "●", group: "◍" };

function SlackWorkspace({ paneId, active, status }: { paneId: string; active: boolean; status: SlackStatus }) {
    const view = useSlackView(paneId);
    const workspace =
        status.workspaces.find((each) => each.id === view.workspace) ?? status.workspaces.find((each) => each.isDefault) ?? status.workspaces[0];
    const channels = useResourceEnabled(active && !!workspace, slackChannelsR, workspace?.id ?? "");
    const [filter, setFilter] = useState("");
    const [link, setLink] = useState("");
    const [opening, setOpening] = useState(false);
    if (!workspace) return null;

    const words = filter.trim().toLowerCase();
    const shown = (channels.data ?? []).filter((channel) => !words || channel.name.toLowerCase().includes(words));
    const rooms = shown.filter((channel) => channel.kind === "channel" || channel.kind === "private");
    const people = shown.filter((channel) => channel.kind === "dm" || channel.kind === "group");
    const current = channels.data?.find((channel) => channel.id === view.channel) ?? null;

    const openLink = async () => {
        if (!link.trim() || opening) return;
        setOpening(true);
        try {
            const thread = await slackApi.threadOf(link.trim());
            updateSlackView(paneId, { workspace: thread.workspace, channel: thread.channel, thread: { channel: thread.channel, ts: thread.ts } });
            setLink("");
        } catch (error) {
            notify("error", `Open the link: ${failureMessage(error)}`);
        } finally {
            setOpening(false);
        }
    };

    const row = (channel: SlackChannel) => (
        <button
            key={channel.id}
            type="button"
            className={`slack-channel${channel.id === view.channel ? " active" : ""}`}
            onClick={() => updateSlackView(paneId, { channel: channel.id, thread: null })}>
            <span className="slack-channel-mark" aria-hidden="true">
                {KIND_MARK[channel.kind]}
            </span>
            <span className="slack-channel-name">{channel.name}</span>
        </button>
    );

    return (
        <div className={`slack-pane${view.thread ? " with-thread" : ""}`}>
            <nav className="slack-sidebar" aria-label="Slack channels">
                {status.workspaces.length > 1 ? (
                    <Dropdown
                        label="Workspace"
                        value={workspace.id}
                        options={status.workspaces.map((each) => ({ value: each.id, label: each.name, detail: each.domain }))}
                        onChange={(id) => updateSlackView(paneId, { workspace: id, channel: null, thread: null })}
                    />
                ) : (
                    <div className="slack-workspace" title={workspace.domain}>
                        {workspace.name}
                    </div>
                )}
                <input
                    className="slack-input"
                    value={link}
                    onChange={(event) => setLink(event.target.value)}
                    onKeyDown={(event) => {
                        if (event.key === "Enter") void openLink();
                    }}
                    placeholder={opening ? "Opening…" : "Paste a Slack message link"}
                    aria-label="Open a Slack message link"
                    spellCheck={false}
                />
                <input
                    className="slack-input"
                    value={filter}
                    onChange={(event) => setFilter(event.target.value)}
                    placeholder="Find a channel or person"
                    aria-label="Find a channel or person"
                    spellCheck={false}
                />
                {channels.status === "error" ? (
                    <p className="slack-note">{channels.error}</p>
                ) : !channels.data ? (
                    <SkeletonRows rows={6} label="Loading channels" />
                ) : (
                    <div className="slack-channels">
                        {rooms.length > 0 && <div className="slack-heading">Channels</div>}
                        {rooms.map(row)}
                        {people.length > 0 && <div className="slack-heading">Direct messages</div>}
                        {people.map(row)}
                    </div>
                )}
                <button
                    type="button"
                    className="slack-signout"
                    onClick={() => void slackApi.signOut(workspace.id).then(refreshSlack).catch(reportError("sign out of Slack"))}>
                    Sign out of {workspace.name}
                </button>
            </nav>
            {view.channel ? (
                <ChannelView
                    active={active}
                    workspace={workspace.id}
                    channel={view.channel}
                    title={current ? `${KIND_MARK[current.kind] === "#" ? "#" : ""}${current.name}` : view.channel}
                    openThread={view.thread?.ts ?? null}
                    onThread={(ts) => updateSlackView(paneId, { thread: ts && view.channel ? { channel: view.channel, ts } : null })}
                />
            ) : (
                <div className="slack-main">
                    <EmptyState message="Pick a channel, or paste a link to a message to read its thread." />
                </div>
            )}
            {view.thread && (
                <ThreadView
                    active={active}
                    workspace={workspace.id}
                    channel={view.thread.channel}
                    ts={view.thread.ts}
                    channelName={current?.name ?? null}
                    onClose={() => updateSlackView(paneId, { thread: null })}
                />
            )}
        </div>
    );
}

function MessageView({ message, onThread, open }: { message: SlackMessage; onThread?: (ts: string) => void; open?: boolean }) {
    return (
        <article className={`slack-message${open ? " open" : ""}`} aria-label={`${message.name} at ${when(message.at)}`}>
            <header className="slack-message-head">
                <span className="slack-avatar" aria-hidden="true">
                    {message.name.slice(0, 1).toUpperCase()}
                </span>
                <span className="slack-author">{message.name}</span>
                {message.bot && <span className="slack-badge">app</span>}
                <span className="slack-time">{when(message.at)}</span>
            </header>
            <Markdown className="prose slack-text">{message.text || " "}</Markdown>
            {message.files.length > 0 && <div className="slack-files">📎 {message.files.join(", ")}</div>}
            {onThread && message.replyCount > 0 && (
                <button type="button" className="slack-replies" onClick={() => onThread(message.threadTs ?? message.ts)}>
                    {message.replyCount === 1 ? "1 reply" : `${message.replyCount} replies`}
                </button>
            )}
        </article>
    );
}

function ChannelView({
    active,
    workspace,
    channel,
    title,
    openThread,
    onThread,
}: {
    active: boolean;
    workspace: string;
    channel: string;
    title: string;
    openThread: string | null;
    onThread: (ts: string | null) => void;
}) {
    const history = useResourceEnabled(active, slackHistoryR, workspace, channel);
    return (
        <section className="slack-main" aria-label={title}>
            <header className="slack-main-head">
                <h2>{title}</h2>
                <button type="button" className="slack-chip" onClick={() => void history.refresh()}>
                    Refresh
                </button>
            </header>
            <div className="slack-messages">
                {history.status === "error" ? (
                    <EmptyState message={history.error ?? "Slack could not read this channel."} tone="error" />
                ) : !history.data ? (
                    <SkeletonRows rows={6} label="Loading messages" />
                ) : history.data.length === 0 ? (
                    <EmptyState message="No messages here yet." />
                ) : (
                    history.data.map((message) => (
                        <MessageView key={message.ts} message={message} open={(message.threadTs ?? message.ts) === openThread} onThread={onThread} />
                    ))
                )}
            </div>
        </section>
    );
}

function ThreadView({
    active,
    workspace,
    channel,
    ts,
    channelName,
    onClose,
}: {
    active: boolean;
    workspace: string;
    channel: string;
    ts: string;
    channelName: string | null;
    onClose: () => void;
}) {
    const thread = useResourceEnabled(active, slackThreadR, workspace, channel, ts);
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
    const [draft, setDraft] = useState("");
    const [sending, setSending] = useState(false);

    const reply = async () => {
        if (!draft.trim() || sending) return;
        setSending(true);
        try {
            await slackApi.reply(workspace, channel, ts, draft.trim());
            setDraft("");
            invalidate((kind) => kind === "slack.thread" || kind === "slack.history");
        } catch (error) {
            notify("error", `Reply: ${failureMessage(error)}`);
        } finally {
            setSending(false);
        }
    };

    const found = thread.data;
    return (
        <aside className="slack-thread" aria-label="Thread">
            <header className="slack-main-head">
                <h2>Thread</h2>
                <span className="slack-grow" />
                <button
                    type="button"
                    className="slack-chip primary"
                    disabled={!found}
                    title="Hand this thread to an agent as context"
                    onClick={(event) => {
                        const box = event.currentTarget.getBoundingClientRect();
                        setMenu({ x: box.right, y: box.bottom + 4 });
                    }}>
                    Send to agent
                </button>
                {found?.permalink && (
                    <>
                        <button
                            type="button"
                            className="slack-chip"
                            onClick={() =>
                                void copyText(found.permalink ?? "").then(() => notify("success", "copied the link"), reportError("copy"))
                            }>
                            Copy link
                        </button>
                        <button type="button" className="slack-chip" onClick={() => void openUrl(found.permalink ?? "").catch(swallow("open Slack"))}>
                            Open in Slack
                        </button>
                    </>
                )}
                <button type="button" className="slack-chip" aria-label="Close the thread" onClick={onClose}>
                    ✕
                </button>
            </header>
            <div className="slack-messages">
                {thread.status === "error" ? (
                    <EmptyState message={thread.error ?? "Slack could not read this thread."} tone="error" />
                ) : !found ? (
                    <SkeletonRows rows={4} label="Loading the thread" />
                ) : (
                    found.messages.map((message) => <MessageView key={message.ts} message={message} />)
                )}
            </div>
            <div className="slack-composer">
                <textarea
                    value={draft}
                    onChange={(event) => setDraft(event.target.value)}
                    onKeyDown={(event) => {
                        if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) void reply();
                    }}
                    placeholder="Reply in the thread (⌘↵ to send)"
                    aria-label="Reply in the thread"
                />
                <button type="button" className="slack-chip primary" disabled={!draft.trim() || sending} onClick={() => void reply()}>
                    {sending ? "Sending…" : "Reply"}
                </button>
            </div>
            {menu && found && (
                <SendToAgentMenu
                    x={menu.x}
                    y={menu.y}
                    alignRight
                    delivery={() => ({ text: threadForAgent(found, channelName) })}
                    onClose={() => setMenu(null)}
                />
            )}
        </aside>
    );
}
