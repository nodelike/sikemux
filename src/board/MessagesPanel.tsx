import { useEffect, useState } from "react";
import { kindLabel, outcomeLabel, rowKeys, whoLabel, type AgentMessageRows, type MessageRow } from "../agents/agentMessages";
import { AgentMark } from "../agents/AgentMark";
import { inboxApi } from "../api/inbox";
import { formatAge } from "../lib/format";
import { useInbox } from "../rail/useInbox";
import { reportError } from "../state/toast";
import { IconClose } from "../ui/Icons";

/** The project's messages, read again whenever the core says the project logged another. */
function useMessageLog(folder: string): AgentMessageRows | null {
    const [log, setLog] = useState<AgentMessageRows | null>(null);
    const latest = useInbox().inbox.agentMessages[log?.project ?? folder];
    useEffect(() => {
        let current = true;
        inboxApi
            .agentMessageRows(folder)
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
    const rows = log?.rows ?? [];
    const keys = rowKeys(rows);
    const held = rows.filter((row) => row.tone === "held").length;
    const failed = rows.filter((row) => row.tone === "failed").length;
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
            <p className="board-auto-about">Newest first. Sikemux keeps who sent what kind and how it went, never the text.</p>
            {log && rows.length === 0 ? (
                <p className="board-auto-hint">No agent has messaged another here yet.</p>
            ) : (
                <ol className="board-messages">
                    {rows.map((row, index) => (
                        <Message key={keys[index]} row={row} />
                    ))}
                </ol>
            )}
        </aside>
    );
}

function Message({ row }: { row: MessageRow }) {
    return (
        <li className={`board-message${row.tone ? ` ${row.tone}` : ""}`}>
            <span className="board-message-who">
                {row.from && <AgentMark agentId={row.from} cli={null} />}
                <b>{whoLabel(row.from, row.fromName)}</b>
                <span className="board-message-to">to</span>
                <b>{whoLabel(row.to, row.toName)}</b>
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
