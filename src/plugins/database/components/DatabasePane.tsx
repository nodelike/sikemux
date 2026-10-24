import { useRef, useState } from "react";
import { copyWithNotice, reportError } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { readViewsAgain } from "../../../plugin-api/views";
import { EmptyState, IconMinus, IconPlus, IconRefresh, SkeletonRows } from "../../../plugin-api/ui";
import { databaseApi } from "../api";
import { loadQuery, runQuery } from "../queryState";
import { DATABASE_PLUGIN } from "../kinds";
import { database, databaseExplorerV, databaseFormR } from "../resources";
import {
    closeConnectionTabs,
    closeDatabaseTab,
    closeOtherDatabaseTabs,
    collapseAll,
    expandNodes,
    readDatabaseView,
    showTab,
    toggleNode,
    updateDatabaseView,
    useDatabaseView,
} from "../state";
import { connectionTab, consoleFor, historyTab, newConsole, tableTab } from "../tabs";
import type { DatabaseConnection, DatabaseExplorer } from "../types.generated";
import { DatabaseMark } from "./DatabaseMark";
import { Explorer, openNodes, tableKey, type ExplorerActions } from "./Explorer";
import { ProfileForm } from "./ProfileForm";
import { TabBody } from "./TabBody";
import { TabStrip } from "./TabStrip";
import "../database.css";

export function DatabasePane({ paneId, active }: { paneId: string; active: boolean }) {
    const view = useDatabaseView(paneId);
    const explorer = useResourceEnabled(active, databaseExplorerV, `${paneId}/explorer`, openNodes(view.expanded));
    const editingSaved = view.editing === "new" ? null : view.editing;
    const form = useResourceEnabled(active && view.editing !== null, databaseFormR, editingSaved ?? "");
    const [filter, setFilter] = useState("");
    const shown = useRef<DatabaseExplorer | undefined>(undefined);
    if (explorer.data) shown.current = explorer.data;

    if (!shown.current && explorer.status === "error")
        return <EmptyState message={explorer.error ?? "Sikemux could not read the saved connections."} tone="error" />;
    if (!shown.current) return <SkeletonRows rows={4} label="Loading connections" />;

    const list = shown.current.connections;
    const open = (change: Parameters<typeof updateDatabaseView>[1]) => updateDatabaseView(paneId, change);
    const connectionOf = (id: string) => list.find((entry) => entry.id === id) ?? null;

    const putInConsole = (profile: DatabaseConnection, sql: string, run: boolean) => {
        const tab = consoleFor(readDatabaseView(paneId), profile.id);
        showTab(paneId, tab);
        loadQuery(tab.id, sql);
        if (run) void runQuery(tab.id, profile.id, sql);
    };
    const actions: ExplorerActions = {
        connect: async (profile) => {
            await databaseApi.connect(profile.id);
            if (readDatabaseView(paneId).tabs.length === 0) showTab(paneId, consoleFor(readDatabaseView(paneId), profile.id));
        },
        disconnect: (profile) => databaseApi.disconnect(profile.id).catch(reportError("disconnect")),
        refresh: () => void readViewsAgain(DATABASE_PLUGIN).catch(reportError("refresh the explorer")),
        newConsole: (profile) => showTab(paneId, newConsole(readDatabaseView(paneId), profile.id)),
        history: (profile) => showTab(paneId, historyTab(profile.id)),
        properties: (profile) => showTab(paneId, connectionTab(profile.id)),
        edit: (profile) => open({ editing: profile.id }),
        openTable: (profile, schema, table) => showTab(paneId, tableTab(profile.id, schema, table)),
        queryTable: (profile, sql, run) => putInConsole(profile, sql, run),
        copy: (text, what) => copyWithNotice(text, `the ${what}`),
    };

    if (list.length === 0 && view.editing !== "new") {
        return (
            <div className="db-pane db-pane-empty">
                <div className="db-welcome">
                    <DatabaseMark size={30} />
                    <h2>Connect a database</h2>
                    <p>
                        Save a PostgreSQL or MySQL server, or a SQLite file, to browse its tables and run SQL beside your code. Your agents can use it
                        too.
                    </p>
                    <button type="button" className="db-button primary" onClick={() => open({ editing: "new" })}>
                        Add a connection
                    </button>
                    <p className="db-hint">Passwords stay in the macOS Keychain.</p>
                </div>
            </div>
        );
    }

    const saved = () => {
        database.refresh(databaseFormR);
        open({ editing: null });
    };
    const tab = view.tabs.find((each) => each.id === view.active) ?? null;
    const tabProfile = tab ? connectionOf(tab.profile) : null;
    const consoleProfile = [tabProfile, ...list].find((profile) => profile?.connected) ?? null;

    return (
        <div className="db-pane">
            <nav className="db-sidebar" aria-label="Database explorer">
                <div className="db-sidebar-head">
                    <span className="db-heading">Explorer</span>
                    <span className="db-grow" />
                    <button type="button" className="db-icon-button" title="Refresh" aria-label="Refresh" onClick={actions.refresh}>
                        <IconRefresh size={13} />
                    </button>
                    <button
                        type="button"
                        className="db-icon-button"
                        title="Collapse all"
                        aria-label="Collapse all"
                        onClick={() => collapseAll(paneId)}>
                        <IconMinus size={13} />
                    </button>
                    <button
                        type="button"
                        className="db-icon-button"
                        title="New connection"
                        aria-label="New connection"
                        onClick={() => open({ editing: "new" })}>
                        <IconPlus size={13} />
                    </button>
                </div>
                <input
                    className="db-filter"
                    value={filter}
                    onChange={(event) => setFilter(event.target.value)}
                    placeholder="Filter tables"
                    aria-label="Filter tables"
                    spellCheck={false}
                />
                <Explorer
                    connections={list}
                    expanded={view.expanded}
                    filter={filter}
                    current={tab?.kind === "table" ? tableKey(tab.profile, tab.schema, tab.name) : null}
                    onToggle={(key) => toggleNode(paneId, key)}
                    onExpand={(keys) => expandNodes(paneId, keys)}
                    actions={actions}
                />
            </nav>
            <section className="db-main">
                {view.editing !== null && form.status === "error" ? (
                    <EmptyState message={form.error ?? "Sikemux could not read the connection."} tone="error" />
                ) : view.editing !== null && !form.data ? (
                    <SkeletonRows rows={4} label="Loading the connection" />
                ) : view.editing !== null && form.data ? (
                    <ProfileForm
                        key={view.editing}
                        form={form.data}
                        onSaved={saved}
                        onRemoved={() => (editingSaved ? closeConnectionTabs(paneId, editingSaved) : open({ editing: null }))}
                        onCancel={() => open({ editing: null })}
                    />
                ) : (
                    <>
                        {view.tabs.length > 0 && (
                            <TabStrip
                                tabs={view.tabs}
                                active={view.active}
                                connections={list}
                                onSelect={(id) => open({ active: id })}
                                onClose={(id) => closeDatabaseTab(paneId, id)}
                                onCloseOthers={(id) => closeOtherDatabaseTabs(paneId, id)}
                                onNewConsole={consoleProfile ? () => actions.newConsole(consoleProfile) : undefined}
                            />
                        )}
                        <div className="db-main-body">
                            {tab && tabProfile ? (
                                <TabBody
                                    key={tab.id}
                                    slot={`${paneId}/tab`}
                                    tab={tab}
                                    profile={tabProfile}
                                    active={active}
                                    onQuery={(sql, run) => putInConsole(tabProfile, sql, run)}
                                    onOpenTable={(schema, table) => actions.openTable(tabProfile, schema, table)}
                                    onEdit={() => actions.edit(tabProfile)}
                                />
                            ) : (
                                <EmptyState message="Open a connection in the explorer to browse its tables, or double-click it for a console." />
                            )}
                        </div>
                    </>
                )}
            </section>
        </div>
    );
}
