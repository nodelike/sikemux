import { useState, type MouseEvent, type ReactNode } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { ContextMenu, IconChevron, IconFolder, type ContextMenuItem } from "../../../plugin-api/ui";
import { failureMessage, type Connected, type DatabaseProfile, type DatabaseTable } from "../api";
import { addressOf, defaultSchema, engineLabel } from "../profileForm";
import { databaseSchemasR, databaseTableR, databaseTablesR } from "../resources";
import { DatabaseMark } from "./DatabaseMark";

export type TableGroup = "tables" | "views";

export const connectionKey = (profile: string) => `c:${profile}`;
export const schemaKey = (profile: string, schema: string) => `s:${profile}:${schema}`;
export const groupKey = (profile: string, schema: string, group: TableGroup) => `g:${profile}:${schema}:${group}`;
export const tableKey = (profile: string, schema: string, table: string) => `t:${profile}:${schema}:${table}`;

const GROUP_LABEL: Record<TableGroup, string> = { tables: "tables", views: "views" };

export const groupOf = (table: DatabaseTable): TableGroup => (table.kind === "table" || table.kind === "foreign-table" ? "tables" : "views");

export interface ExplorerActions {
    connect: (profile: DatabaseProfile) => Promise<void>;
    disconnect: (profile: DatabaseProfile) => Promise<void>;
    refresh: (profile: DatabaseProfile) => void;
    newConsole: (profile: DatabaseProfile) => void;
    history: (profile: DatabaseProfile) => void;
    properties: (profile: DatabaseProfile) => void;
    edit: (profile: DatabaseProfile) => void;
    openTable: (profile: DatabaseProfile, schema: string, table: string) => void;
    queryTable: (profile: DatabaseProfile, schema: string, table: string, run: boolean) => void;
    copy: (text: string, what: string) => void;
}

interface Tree {
    active: boolean;
    expanded: Set<string>;
    filter: string;
    /** The table whose tab is in front, by node key. */
    current: string | null;
    onToggle: (key: string) => void;
    onExpand: (keys: string[]) => void;
    actions: ExplorerActions;
    menu: (event: MouseEvent, items: ContextMenuItem[]) => void;
}

/** Every saved connection as a tree: connection, schema, tables and views, then each table's columns. */
export function Explorer({
    profiles,
    connected,
    ...tree
}: Omit<Tree, "menu" | "expanded"> & { profiles: DatabaseProfile[]; connected: Connected[]; expanded: string[] }) {
    const [menu, setMenu] = useState<{ x: number; y: number; items: ContextMenuItem[] } | null>(null);
    const shared: Tree = {
        ...tree,
        expanded: new Set(tree.expanded),
        menu: (event, items) => {
            event.preventDefault();
            setMenu({ x: event.clientX, y: event.clientY, items });
        },
    };
    return (
        <>
            <ul className="db-explorer" role="tree" aria-label="Database explorer">
                {profiles.map((profile) => (
                    <ConnectionNode
                        key={profile.id}
                        profile={profile}
                        connected={connected.find((entry) => entry.id === profile.id) ?? null}
                        tree={shared}
                    />
                ))}
            </ul>
            {menu && <ContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={() => setMenu(null)} />}
        </>
    );
}

function Row({
    depth,
    open,
    current,
    icon,
    label,
    detail,
    count,
    title,
    onClick,
    onDoubleClick,
    onContextMenu,
}: {
    depth: number;
    /** Whether the node is open; undefined for a leaf. */
    open?: boolean;
    current?: boolean;
    icon: ReactNode;
    label: ReactNode;
    detail?: ReactNode;
    count?: string | null;
    title?: string;
    onClick?: () => void;
    onDoubleClick?: () => void;
    onContextMenu?: (event: MouseEvent) => void;
}) {
    return (
        <button
            type="button"
            className={`db-node${current ? " current" : ""}`}
            style={{ paddingLeft: 6 + depth * 16 }}
            title={title}
            onClick={onClick}
            onDoubleClick={onDoubleClick}
            onContextMenu={onContextMenu}>
            <span className={`db-node-chevron${open ? " open" : ""}`} aria-hidden="true">
                {open !== undefined && <IconChevron size={11} />}
            </span>
            <span className="db-node-icon" aria-hidden="true">
                {icon}
            </span>
            <span className="db-node-label">{label}</span>
            {count && <span className="db-node-count">{count}</span>}
            {detail && <span className="db-node-detail">{detail}</span>}
        </button>
    );
}

function ConnectionNode({ profile, connected, tree }: { profile: DatabaseProfile; connected: Connected | null; tree: Tree }) {
    const key = connectionKey(profile.id);
    const [connecting, setConnecting] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const open = tree.expanded.has(key) && !!connected;
    const schemas = useResourceEnabled(tree.active && !!connected, databaseSchemasR, profile.id);
    const usual = defaultSchema(profile);

    const connect = async () => {
        setConnecting(true);
        setError(null);
        try {
            await tree.actions.connect(profile);
            tree.onExpand([key, ...(usual ? [schemaKey(profile.id, usual), groupKey(profile.id, usual, "tables")] : [])]);
        } catch (failure) {
            setError(failureMessage(failure));
        } finally {
            setConnecting(false);
        }
    };
    const toggle = () => {
        if (connected) tree.onToggle(key);
        else if (!connecting) void connect();
    };
    const items: ContextMenuItem[] = [
        connected ? { label: "Disconnect", run: () => void tree.actions.disconnect(profile) } : { label: "Connect", run: () => void connect() },
        { label: "New console", disabled: !connected, run: () => tree.actions.newConsole(profile) },
        { label: "History", run: () => tree.actions.history(profile) },
        { label: "Refresh", disabled: !connected, run: () => tree.actions.refresh(profile) },
        { sep: true },
        { label: "Properties", run: () => tree.actions.properties(profile) },
        { label: "Edit connection…", run: () => tree.actions.edit(profile) },
        { label: "Copy name", run: () => tree.actions.copy(profile.name, "name") },
    ];

    return (
        <li role="treeitem" aria-expanded={open} aria-label={profile.name}>
            <Row
                depth={0}
                open={open}
                icon={<DatabaseMark size={14} />}
                label={profile.name}
                count={connected && schemas.data ? String(schemas.data.length) : null}
                detail={
                    connecting ? (
                        "connecting…"
                    ) : connected ? (
                        <span className="db-dot" title={`Connected to ${connected.version}`} aria-label="Connected" />
                    ) : null
                }
                title={`${engineLabel(profile.engine)} · ${addressOf(profile)}`}
                onClick={toggle}
                onDoubleClick={() => connected && tree.actions.newConsole(profile)}
                onContextMenu={(event) => tree.menu(event, items)}
            />
            {error && (
                <div className="db-node-error" role="alert" style={{ paddingLeft: 38 }}>
                    {error}
                </div>
            )}
            {open && (
                <ul role="group">
                    {schemas.status === "error" ? (
                        <NodeNote depth={1} error>
                            {schemas.error}
                        </NodeNote>
                    ) : !schemas.data ? (
                        <NodeNote depth={1}>Loading schemas…</NodeNote>
                    ) : (
                        schemas.data.map((schema) => <SchemaNode key={schema} profile={profile} schema={schema} tree={tree} />)
                    )}
                </ul>
            )}
        </li>
    );
}

function SchemaNode({ profile, schema, tree }: { profile: DatabaseProfile; schema: string; tree: Tree }) {
    const key = schemaKey(profile.id, schema);
    const open = tree.expanded.has(key);
    const tables = useResourceEnabled(tree.active, databaseTablesR, profile.id, schema);
    const groups = (["tables", "views"] as const).map((group) => ({
        group,
        members: (tables.data ?? []).filter((table) => groupOf(table) === group),
    }));
    const words = tree.filter.trim().toLowerCase();

    return (
        <li role="treeitem" aria-expanded={open} aria-label={schema}>
            <Row
                depth={1}
                open={open}
                icon={<span className="db-glyph">◫</span>}
                label={schema}
                count={tables.data ? String(tables.data.length) : null}
                onClick={() => tree.onToggle(key)}
                onContextMenu={(event) =>
                    tree.menu(event, [
                        { label: "New console", run: () => tree.actions.newConsole(profile) },
                        { label: "Refresh", run: () => tree.actions.refresh(profile) },
                        { label: "Copy name", run: () => tree.actions.copy(schema, "schema name") },
                    ])
                }
            />
            {open && (
                <ul role="group">
                    {tables.status === "error" ? (
                        <NodeNote depth={2} error>
                            {tables.error}
                        </NodeNote>
                    ) : !tables.data ? (
                        <NodeNote depth={2}>Loading tables…</NodeNote>
                    ) : tables.data.length === 0 ? (
                        <NodeNote depth={2}>No tables here yet.</NodeNote>
                    ) : (
                        groups
                            .filter(({ members }) => members.length > 0)
                            .map(({ group, members }) => (
                                <GroupNode
                                    key={group}
                                    profile={profile}
                                    schema={schema}
                                    group={group}
                                    members={words ? members.filter((table) => table.name.toLowerCase().includes(words)) : members}
                                    total={members.length}
                                    tree={tree}
                                />
                            ))
                    )}
                </ul>
            )}
        </li>
    );
}

function GroupNode({
    profile,
    schema,
    group,
    members,
    total,
    tree,
}: {
    profile: DatabaseProfile;
    schema: string;
    group: TableGroup;
    members: DatabaseTable[];
    total: number;
    tree: Tree;
}) {
    const key = groupKey(profile.id, schema, group);
    const open = tree.expanded.has(key);
    return (
        <li role="treeitem" aria-expanded={open} aria-label={`${schema} ${GROUP_LABEL[group]}`}>
            <Row
                depth={2}
                open={open}
                icon={<IconFolder size={13} />}
                label={GROUP_LABEL[group]}
                count={members.length === total ? String(total) : `${members.length} of ${total}`}
                onClick={() => tree.onToggle(key)}
            />
            {open && (
                <ul role="group">
                    {members.map((table) => (
                        <TableNode key={table.name} profile={profile} schema={schema} table={table} tree={tree} />
                    ))}
                </ul>
            )}
        </li>
    );
}

function TableNode({ profile, schema, table, tree }: { profile: DatabaseProfile; schema: string; table: DatabaseTable; tree: Tree }) {
    const key = tableKey(profile.id, schema, table.name);
    const open = tree.expanded.has(key);
    const { actions } = tree;
    return (
        <li role="treeitem" aria-expanded={open} aria-label={table.name}>
            <Row
                depth={3}
                open={open}
                current={tree.current === key}
                icon={<span className={`db-glyph ${table.kind}`}>{table.kind === "table" ? "▦" : "◇"}</span>}
                label={table.name}
                title={`${schema}.${table.name}, a ${table.kind.replace("-", " ")}`}
                onClick={() => actions.openTable(profile, schema, table.name)}
                onDoubleClick={() => actions.queryTable(profile, schema, table.name, true)}
                onContextMenu={(event) =>
                    tree.menu(event, [
                        { label: "Open", run: () => actions.openTable(profile, schema, table.name) },
                        { label: "Preview rows", run: () => actions.queryTable(profile, schema, table.name, true) },
                        { label: "Query this table", run: () => actions.queryTable(profile, schema, table.name, false) },
                        { label: open ? "Hide columns" : "Show columns", run: () => tree.onToggle(key) },
                        { sep: true },
                        { label: "Copy name", run: () => actions.copy(table.name, "table name") },
                        { label: "Copy qualified name", run: () => actions.copy(`${schema}.${table.name}`, "qualified name") },
                    ])
                }
            />
            {open && <ColumnNodes profile={profile} schema={schema} table={table.name} tree={tree} />}
        </li>
    );
}

function ColumnNodes({ profile, schema, table, tree }: { profile: DatabaseProfile; schema: string; table: string; tree: Tree }) {
    const info = useResourceEnabled(tree.active, databaseTableR, profile.id, schema, table);
    return (
        <ul role="group">
            {info.status === "error" ? (
                <NodeNote depth={4} error>
                    {info.error}
                </NodeNote>
            ) : !info.data ? (
                <NodeNote depth={4}>Loading columns…</NodeNote>
            ) : (
                info.data.columns.map((column) => (
                    <li key={column.name} role="treeitem" aria-label={column.name}>
                        <Row
                            depth={4}
                            icon={<span className={`db-glyph column${column.primaryKey ? " key" : ""}`}>{column.primaryKey ? "⚿" : "▫"}</span>}
                            label={column.name}
                            detail={<span className="db-node-type">{column.type}</span>}
                            title={`${column.name} ${column.type}${column.nullable ? "" : " not null"}${column.primaryKey ? ", primary key" : ""}`}
                            onContextMenu={(event) =>
                                tree.menu(event, [{ label: "Copy name", run: () => tree.actions.copy(column.name, "column name") }])
                            }
                        />
                    </li>
                ))
            )}
        </ul>
    );
}

function NodeNote({ depth, error, children }: { depth: number; error?: boolean; children: ReactNode }) {
    return (
        <li className={`db-node-note${error ? " error" : ""}`} role={error ? "alert" : undefined} style={{ paddingLeft: 6 + depth * 16 + 32 }}>
            {children}
        </li>
    );
}
