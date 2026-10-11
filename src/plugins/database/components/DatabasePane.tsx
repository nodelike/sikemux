import { useState } from "react";
import { copyText, notify, reportError } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, IconMinus, IconPlus, IconRefresh, SkeletonRows } from "../../../plugin-api/ui";
import { databaseApi, refreshDatabase, type DatabaseProfile } from "../api";
import { blankDraft, draftOf } from "../profileForm";
import { loadQuery, runQuery } from "../queryState";
import { databaseConnectedR, databaseProfilesR } from "../resources";
import { previewSql } from "../sql";
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
import { DatabaseMark } from "./DatabaseMark";
import { Explorer, tableKey, type ExplorerActions } from "./Explorer";
import { ProfileForm } from "./ProfileForm";
import { TabBody } from "./TabBody";
import { TabStrip } from "./TabStrip";
import "../database.css";

export function DatabasePane({ paneId, active }: { paneId: string; active: boolean }) {
    const profiles = useResourceEnabled(active, databaseProfilesR);
    const connected = useResourceEnabled(active, databaseConnectedR);
    const view = useDatabaseView(paneId);
    const [filter, setFilter] = useState("");

    if (profiles.status === "error") return <EmptyState message={profiles.error ?? "Sikemux could not read the saved connections."} tone="error" />;
    if (!profiles.data) return <SkeletonRows rows={4} label="Loading connections" />;

    const list = profiles.data;
    const open = (change: Parameters<typeof updateDatabaseView>[1]) => updateDatabaseView(paneId, change);
    const connections = connected.data ?? [];
    const connectionOf = (id: string) => connections.find((entry) => entry.id === id) ?? null;

    const putInConsole = (profile: DatabaseProfile, sql: string, run: boolean) => {
        const tab = consoleFor(readDatabaseView(paneId), profile.id);
        showTab(paneId, tab);
        loadQuery(tab.id, sql);
        if (run) void runQuery(tab.id, profile.id, sql);
    };
    const actions: ExplorerActions = {
        connect: async (profile) => {
            await databaseApi.connect(profile.id);
            refreshDatabase();
            if (readDatabaseView(paneId).tabs.length === 0) showTab(paneId, consoleFor(readDatabaseView(paneId), profile.id));
        },
        disconnect: (profile) => databaseApi.disconnect(profile.id).then(refreshDatabase, reportError("disconnect")),
        refresh: () => refreshDatabase(),
        newConsole: (profile) => showTab(paneId, newConsole(readDatabaseView(paneId), profile.id)),
        history: (profile) => showTab(paneId, historyTab(profile.id)),
        properties: (profile) => showTab(paneId, connectionTab(profile.id)),
        edit: (profile) => open({ editing: profile.id }),
        openTable: (profile, schema, table) => showTab(paneId, tableTab(profile.id, schema, table)),
        queryTable: (profile, schema, table, run) =>
            putInConsole(profile, previewSql(profile.engine, profile.engine === "sqlite" && schema === "main" ? "" : schema, table), run),
        copy: (text, what) => void copyText(text).then(() => notify("success", `copied the ${what}`), reportError("copy")),
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

    const editing = view.editing === "new" ? null : (list.find((profile) => profile.id === view.editing) ?? null);
    const tab = view.tabs.find((each) => each.id === view.active) ?? null;
    const tabProfile = tab ? (list.find((profile) => profile.id === tab.profile) ?? null) : null;
    const consoleProfile = [tabProfile, ...list].find((profile) => profile && connectionOf(profile.id)) ?? null;

    return (
        <div className="db-pane">
            <nav className="db-sidebar" aria-label="Database explorer">
                <div className="db-sidebar-head">
                    <span className="db-heading">Explorer</span>
                    <span className="db-grow" />
                    <button type="button" className="db-icon-button" title="Refresh" aria-label="Refresh" onClick={() => refreshDatabase()}>
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
                    profiles={list}
                    connected={connections}
                    active={active}
                    expanded={view.expanded}
                    filter={filter}
                    current={tab?.kind === "table" ? tableKey(tab.profile, tab.schema, tab.name) : null}
                    onToggle={(key) => toggleNode(paneId, key)}
                    onExpand={(keys) => expandNodes(paneId, keys)}
                    actions={actions}
                />
            </nav>
            <section className="db-main">
                {view.editing === "new" ? (
                    <ProfileForm
                        key="new"
                        initial={blankDraft()}
                        saved={null}
                        onSaved={() => {
                            refreshDatabase();
                            open({ editing: null });
                        }}
                        onRemoved={() => open({ editing: null })}
                        onCancel={() => open({ editing: null })}
                    />
                ) : editing ? (
                    <ProfileForm
                        key={editing.id}
                        initial={draftOf(editing)}
                        saved={editing}
                        onSaved={() => {
                            refreshDatabase();
                            open({ editing: null });
                        }}
                        onRemoved={() => {
                            closeConnectionTabs(paneId, editing.id);
                            refreshDatabase();
                        }}
                        onCancel={() => open({ editing: null })}
                    />
                ) : (
                    <>
                        {view.tabs.length > 0 && (
                            <TabStrip
                                tabs={view.tabs}
                                active={view.active}
                                profiles={list}
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
                                    tab={tab}
                                    profile={tabProfile}
                                    connected={connectionOf(tabProfile.id)}
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
