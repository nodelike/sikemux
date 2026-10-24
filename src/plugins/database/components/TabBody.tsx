import type { DatabaseTab } from "../tabs";
import type { DatabaseConnection } from "../types.generated";
import { HistoryPanel } from "./HistoryPanel";
import { ProfileDetail } from "./ProfileDetail";
import { QueryWorkspace } from "./QueryWorkspace";
import { SendResults } from "./SendResults";
import { TableView } from "./TableView";

/** What the tab in front shows. A console or table on a closed connection offers to connect first. */
export function TabBody({
    slot,
    tab,
    profile,
    active,
    onQuery,
    onOpenTable,
    onEdit,
}: {
    /** Where the pane shows it, so the core keeps its views for this pane. */
    slot: string;
    tab: DatabaseTab;
    profile: DatabaseConnection;
    active: boolean;
    /** Puts SQL in a console of this connection, running it when asked. */
    onQuery: (sql: string, run: boolean) => void;
    onOpenTable: (schema: string, table: string) => void;
    onEdit: () => void;
}) {
    if (tab.kind === "history") return <HistoryPanel slot={slot} profile={profile} active={active} onOpen={onQuery} />;
    if (tab.kind === "connection" || profile.connected === null) return <ProfileDetail profile={profile} onEdit={onEdit} />;
    if (tab.kind === "table")
        return (
            <TableView
                slot={slot}
                profile={profile}
                table={{ schema: tab.schema, name: tab.name }}
                active={active}
                onQuery={onQuery}
                onOpenTable={(table) => onOpenTable(table.schema, table.name)}
            />
        );
    return <Console id={tab.id} profile={profile} />;
}

function Console({ id, profile }: { id: string; profile: DatabaseConnection }) {
    return (
        <div className="db-console">
            <div className="db-console-head">
                <span className="db-row-name">{profile.name}</span>
                <span className="db-meta">{profile.connected}</span>
            </div>
            <QueryWorkspace consoleId={id} profile={profile} resultActions={(result) => <SendResults result={result} />} />
        </div>
    );
}
