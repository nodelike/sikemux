import { useEffect, useMemo, useState } from "react";
import { kindLabel, messageRows, messageTone, outcomeLabel, type AgentMessageLog, type MessageRow } from "../agents/agentMessages";
import { AgentMark } from "../agents/AgentMark";
import { inboxApi } from "../api/inbox";
import { formatAge } from "../lib/format";
import { useInbox } from "../rail/useInbox";
import { useStore } from "../state/store";
import { reportError } from "../state/toast";
import { IconClose } from "../ui/Icons";

/** The project's log, read again whenever the core says the project logged another message. */
function useMessageLog(folder: string): AgentMessageLog | null {
    const [log, setLog] = useState<AgentMessageLog | null>(null);
    const latest = useInbox().inbox.agentMessages[log?.project ?? folder];
    useEffect(() => {
        let current = true;
        inboxApi
            .agentMessages(folder)
            .then((read) => {
                if (current) setLog(read);
            })
            .catch(reportError("read the messages between agents"));
        return () => {
            current = false;
        };
    }, [folder, latest]);
    return log;
}

/** Who messaged whom in the project, and what became of each message, opened beside the board. */
export function MessagesPanel({ folder, onClose }: { folder: string; onClose: () => void }) {
    const log = useMessageLog(folder);
    const agents = useStore((s) => s.agents);
    const rows = useMemo(() => (log ? messageRows(log.entries, (id) => agents[id]?.name) : []), [log, agents]);
    const held = rows.filter((row) => messageTone(row.outcome) === "held").length;
    const failed = rows.filter((row) => messageTone(row.outcome) === "failed").length;
    return (
        <aside className="board-card-panel" aria-label="Messages between agents">
            <header className="board-panel-head">
                <span className="board-panel-name">Messages</span>
                <span className="kanban-meta">{[held && `${held} waiting`, failed && `${failed} not delivered`].filter(Boolean).join(" · ")}</span>
                <span className="kanban-grow" />
                <button type="button" className="kanban-icon-button" title="Close" aria-label="Close Messages" onClick={onClose}>
                    <IconClose size={12} />
                </button>
            </header>
            <p className="board-auto-about">
                What this project's agents sent each other, newest first. Only who, what kind and how it went is kept, never the text.
            </p>
            {log && rows.length === 0 ? (
                <p className="board-auto-hint">No agent has messaged another here yet.</p>
            ) : (
                <ol className="board-messages">
                    {rows.map((row) => (
                        <Message key={row.key} row={row} />
                    ))}
                </ol>
            )}
        </aside>
    );
}

function Message({ row }: { row: MessageRow }) {
    const cli = useStore((s) => (row.from ? s.agents[row.from]?.type : undefined));
    const tone = messageTone(row.outcome);
    return (
        <li className={`board-message${tone ? ` ${tone}` : ""}`}>
            <span className="board-message-who">
                {cli && <AgentMark cli={cli} />}
                <b>{row.fromName}</b>
                <span className="board-message-to">to</span>
                <b>{row.toName}</b>
                <span className="kanban-grow" />
                <time className="board-message-at" dateTime={new Date(row.sentAt).toISOString()} title={new Date(row.at).toLocaleString()}>
                    {formatAge(row.sentAt)}
                </time>
            </span>
            <span className="board-message-what">
                {kindLabel(row.kind)} · <span className="board-message-outcome">{outcomeLabel(row)}</span>
            </span>
        </li>
    );
}
