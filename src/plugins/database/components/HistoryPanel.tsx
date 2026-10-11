import { useState } from "react";
import { confirmDialog, swallow } from "../../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, SkeletonRows } from "../../../plugin-api/ui";
import { databaseApi, type DatabaseProfile, type HistoryEntry } from "../api";
import { databaseHistoryR } from "../resources";
import { ago, duration } from "../results";

export function HistoryPanel({
    profile,
    active,
    onOpen,
    now = Date.now(),
}: {
    profile: DatabaseProfile;
    active: boolean;
    /** Puts a past query in the editor, running it when asked. */
    onOpen: (sql: string, run: boolean) => void;
    now?: number;
}) {
    const [search, setSearch] = useState("");
    const history = useResourceEnabled(active, databaseHistoryR, profile.id, search.trim());

    const clear = async () => {
        const confirmed = await confirmDialog({
            title: `Clear the history of ${profile.name}?`,
            body: "Every query run here, by you or an agent, is forgotten.",
            confirmLabel: "Clear",
            destructive: true,
        });
        if (!confirmed) return;
        await databaseApi.clearHistory(profile.id);
        invalidate((kind) => kind === "database.history");
    };

    return (
        <section className="db-history" aria-label={`History of ${profile.name}`}>
            <div className="db-toolbar">
                <input
                    className="db-filter db-grow"
                    value={search}
                    onChange={(event) => setSearch(event.target.value)}
                    placeholder="Search past queries"
                    aria-label="Search past queries"
                    spellCheck={false}
                />
                <button
                    type="button"
                    className="db-button"
                    disabled={!history.data?.length}
                    onClick={() => void clear().catch(swallow("clear the history"))}>
                    Clear history
                </button>
            </div>
            {history.status === "error" ? (
                <EmptyState message={history.error ?? "Sikemux could not read the history."} tone="error" />
            ) : !history.data ? (
                <SkeletonRows rows={5} label="Loading history" />
            ) : history.data.length === 0 ? (
                <EmptyState message={search.trim() ? "No past query matches." : "Queries you and your agents run here show up in this list."} />
            ) : (
                <ol className="db-history-list">
                    {history.data.map((entry) => (
                        <HistoryRow key={`${entry.at}-${entry.sql}`} entry={entry} now={now} onOpen={onOpen} />
                    ))}
                </ol>
            )}
        </section>
    );
}

function HistoryRow({ entry, now, onOpen }: { entry: HistoryEntry; now: number; onOpen: (sql: string, run: boolean) => void }) {
    return (
        <li className={`db-history-entry${entry.ok ? "" : " failed"}`}>
            <pre className="db-history-sql">{entry.sql}</pre>
            <div className="db-history-meta">
                <span className="db-meta" title={new Date(entry.at).toLocaleString()}>
                    {ago(entry.at, now)}
                </span>
                <span className="db-meta">{duration(entry.millis)}</span>
                {entry.ok ? (
                    entry.rows !== null && <span className="db-meta">{entry.rows.toLocaleString("en-US")} rows</span>
                ) : (
                    <span className="db-history-error" title={entry.error ?? undefined}>
                        {entry.error ?? "Failed"}
                    </span>
                )}
                {entry.source === "agent" && <span className="db-badge">agent</span>}
                <span className="db-grow" />
                <button type="button" className="db-button" onClick={() => onOpen(entry.sql, false)}>
                    Open
                </button>
                <button type="button" className="db-button" onClick={() => onOpen(entry.sql, true)}>
                    Run again
                </button>
            </div>
        </li>
    );
}
