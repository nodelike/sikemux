import type { Connected, DatabaseProfile } from "../api";
import { useQuery } from "../queryState";
import type { DatabaseTab } from "../tabs";
import { HistoryPanel } from "./HistoryPanel";
import { ProfileDetail } from "./ProfileDetail";
import { QueryWorkspace } from "./QueryWorkspace";
import { SendResults } from "./SendResults";
import { TableView } from "./TableView";

/** What the tab in front shows. A console or table on a closed connection offers to connect first. */
export function TabBody({
    tab,
    profile,
    connected,
    active,
    onQuery,
    onOpenTable,
    onEdit,
}: {
    tab: DatabaseTab;
    profile: DatabaseProfile;
    connected: Connected | null;
    active: boolean;
    /** Puts SQL in a console of this connection, running it when asked. */
    onQuery: (sql: string, run: boolean) => void;
    onOpenTable: (schema: string, table: string) => void;
    onEdit: () => void;
}) {
    if (tab.kind === "history") return <HistoryPanel profile={profile} active={active} onOpen={onQuery} />;
    if (tab.kind === "connection" || !connected) return <ProfileDetail profile={profile} connected={connected} onEdit={onEdit} />;
    if (tab.kind === "table")
        return (
            <TableView
                profile={profile}
                table={{ schema: tab.schema, name: tab.name }}
                active={active}
                onQuery={onQuery}
                onOpenTable={(table) => onOpenTable(table.schema, table.name)}
            />
        );
    return <Console id={tab.id} profile={profile} connected={connected} />;
}

function Console({ id, profile, connected }: { id: string; profile: DatabaseProfile; connected: Connected }) {
    const ran = useQuery(id).ran;
    return (
        <div className="db-console">
            <div className="db-console-head">
                <span className="db-row-name">{profile.name}</span>
                <span className="db-meta">{connected.version}</span>
            </div>
            <QueryWorkspace
                consoleId={id}
                profile={profile}
                resultActions={(result) => <SendResults profile={profile} sql={ran ?? ""} result={result} />}
            />
        </div>
    );
}
