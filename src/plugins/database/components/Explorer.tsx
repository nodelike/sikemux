import { useState, type MouseEvent, type ReactNode } from "react";
import {
    ContextMenu,
    IconChevron,
    IconClock,
    IconCode,
    IconCopy,
    IconEye,
    IconFile,
    IconFolder,
    IconInfo,
    IconPencil,
    IconPlug,
    IconPlus,
    IconPower,
    IconRefresh,
    type ContextMenuItem,
} from "../../../plugin-api/ui";
import { errorText } from "../../../plugin-api/format";
import type { OpenNodes } from "../resources";
import type { DatabaseConnection, DatabaseSchema, DatabaseTableEntry, DatabaseTableGroup } from "../types.generated";
import { DatabaseMark } from "./DatabaseMark";

export const connectionKey = (profile: string) => `c:${profile}`;
export const schemaKey = (profile: string, schema: string) => `s:${profile}:${schema}`;
export const groupKey = (profile: string, schema: string, group: string) => `g:${profile}:${schema}:${group}`;
export const tableKey = (profile: string, schema: string, table: string) => `t:${profile}:${schema}:${table}`;

/** What the explorer view reads, from the nodes the explorer has open. */
export function openNodes(expanded: readonly string[]): OpenNodes[] {
    const ids = expanded.filter((key) => key.startsWith("c:")).map((key) => key.slice(2));
    return ids.sort().map((id) => {
        const schemas = expanded
            .filter((key) => key.startsWith(`s:${id}:`))
            .map((key) => key.slice(`s:${id}:`.length))
            .sort();
        const tables = schemas.flatMap((schema) =>
            expanded
                .filter((key) => key.startsWith(`t:${id}:${schema}:`))
                .map((key) => ({ schema, name: key.slice(`t:${id}:${schema}:`.length) }))
                .sort((a, b) => a.name.localeCompare(b.name)),
        );
        return { id, schemas, tables };
    });
}

export interface ExplorerActions {
    connect: (connection: DatabaseConnection) => Promise<void>;
    disconnect: (connection: DatabaseConnection) => Promise<void>;
    refresh: () => void;
    newConsole: (connection: DatabaseConnection) => void;
    history: (connection: DatabaseConnection) => void;
    properties: (connection: DatabaseConnection) => void;
    edit: (connection: DatabaseConnection) => void;
    openTable: (connection: DatabaseConnection, schema: string, table: string) => void;
    queryTable: (connection: DatabaseConnection, sql: string, run: boolean) => void;
    copy: (text: string, what: string) => void;
}

interface Tree {
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
export function Explorer({ connections, ...tree }: Omit<Tree, "menu" | "expanded"> & { connections: DatabaseConnection[]; expanded: string[] }) {
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
                {connections.map((connection) => (
                    <ConnectionNode key={connection.id} connection={connection} tree={shared} />
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

function ConnectionNode({ connection, tree }: { connection: DatabaseConnection; tree: Tree }) {
    const key = connectionKey(connection.id);
    const [connecting, setConnecting] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const connected = connection.connected !== null;
    const open = tree.expanded.has(key) && connected;
    const usual = connection.defaultSchema;

    const connect = async () => {
        setConnecting(true);
        setError(null);
        try {
            await tree.actions.connect(connection);
            tree.onExpand([key, ...(usual ? [schemaKey(connection.id, usual), groupKey(connection.id, usual, "tables")] : [])]);
        } catch (failure) {
            setError(errorText(failure));
        } finally {
            setConnecting(false);
        }
    };
    const toggle = () => {
        if (connected) tree.onToggle(key);
        else if (!connecting) void connect();
    };
    const items: ContextMenuItem[] = [
        connected
            ? { label: "Disconnect", icon: <IconPower size={13} />, run: () => void tree.actions.disconnect(connection) }
            : { label: "Connect", icon: <IconPlug size={13} />, run: () => void connect() },
        { label: "New console", icon: <IconPlus size={13} />, disabled: !connected, run: () => tree.actions.newConsole(connection) },
        { label: "History", icon: <IconClock size={13} />, run: () => tree.actions.history(connection) },
        { label: "Refresh", icon: <IconRefresh size={13} />, disabled: !connected, run: () => tree.actions.refresh() },
        { sep: true },
        { label: "Properties", icon: <IconInfo size={13} />, run: () => tree.actions.properties(connection) },
        { label: "Edit connection…", icon: <IconPencil size={13} />, run: () => tree.actions.edit(connection) },
        { label: "Copy name", icon: <IconCopy size={13} />, run: () => tree.actions.copy(connection.name, "name") },
    ];

    return (
        <li role="treeitem" aria-expanded={open} aria-label={connection.name}>
            <Row
                depth={0}
                open={open}
                icon={<DatabaseMark size={14} />}
                label={connection.name}
                count={open && connection.schemas ? String(connection.schemas.length) : null}
                detail={
                    connecting ? (
                        "connecting…"
                    ) : connected ? (
                        <span className="db-dot" title={`Connected to ${connection.connected}`} aria-label="Connected" />
                    ) : null
                }
                title={`${connection.engineName} · ${connection.address}`}
                onClick={toggle}
                onDoubleClick={() => connected && tree.actions.newConsole(connection)}
                onContextMenu={(event) => tree.menu(event, items)}
            />
            {error && (
                <div className="db-node-error" role="alert" style={{ paddingLeft: 38 }}>
                    {error}
                </div>
            )}
            {open && (
                <ul role="group">
                    {connection.failed ? (
                        <NodeNote depth={1} error>
                            {connection.failed}
                        </NodeNote>
                    ) : !connection.schemas ? (
                        <NodeNote depth={1}>Loading schemas…</NodeNote>
                    ) : (
                        connection.schemas.map((schema) => <SchemaNode key={schema.name} connection={connection} schema={schema} tree={tree} />)
                    )}
                </ul>
            )}
        </li>
    );
}

function SchemaNode({ connection, schema, tree }: { connection: DatabaseConnection; schema: DatabaseSchema; tree: Tree }) {
    const key = schemaKey(connection.id, schema.name);
    const open = tree.expanded.has(key);
    const words = tree.filter.trim().toLowerCase();
    const groups = schema.groups;
    const count = groups?.reduce((sum, group) => sum + group.tables.length, 0);

    return (
        <li role="treeitem" aria-expanded={open} aria-label={schema.name}>
            <Row
                depth={1}
                open={open}
                icon={<span className="db-glyph">◫</span>}
                label={schema.name}
                count={open && count !== undefined ? String(count) : null}
                onClick={() => tree.onToggle(key)}
                onContextMenu={(event) =>
                    tree.menu(event, [
                        { label: "New console", icon: <IconPlus size={13} />, run: () => tree.actions.newConsole(connection) },
                        { label: "Refresh", icon: <IconRefresh size={13} />, run: () => tree.actions.refresh() },
                        { label: "Copy name", icon: <IconCopy size={13} />, run: () => tree.actions.copy(schema.name, "schema name") },
                    ])
                }
            />
            {open && (
                <ul role="group">
                    {schema.failed ? (
                        <NodeNote depth={2} error>
                            {schema.failed}
                        </NodeNote>
                    ) : !groups ? (
                        <NodeNote depth={2}>Loading tables…</NodeNote>
                    ) : groups.length === 0 ? (
                        <NodeNote depth={2}>No tables here yet.</NodeNote>
                    ) : (
                        groups.map((group) => (
                            <GroupNode
                                key={group.name}
                                connection={connection}
                                schema={schema.name}
                                group={group}
                                members={words ? group.tables.filter((table) => table.name.toLowerCase().includes(words)) : group.tables}
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
    connection,
    schema,
    group,
    members,
    tree,
}: {
    connection: DatabaseConnection;
    schema: string;
    group: DatabaseTableGroup;
    members: DatabaseTableEntry[];
    tree: Tree;
}) {
    const key = groupKey(connection.id, schema, group.name);
    const open = tree.expanded.has(key);
    const total = group.tables.length;
    return (
        <li role="treeitem" aria-expanded={open} aria-label={`${schema} ${group.name}`}>
            <Row
                depth={2}
                open={open}
                icon={<IconFolder size={13} />}
                label={group.name}
                count={members.length === total ? String(total) : `${members.length} of ${total}`}
                onClick={() => tree.onToggle(key)}
            />
            {open && (
                <ul role="group">
                    {members.map((table) => (
                        <TableNode key={table.name} connection={connection} schema={schema} table={table} tree={tree} />
                    ))}
                </ul>
            )}
        </li>
    );
}

function TableNode({ connection, schema, table, tree }: { connection: DatabaseConnection; schema: string; table: DatabaseTableEntry; tree: Tree }) {
    const key = tableKey(connection.id, schema, table.name);
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
                title={`${schema}.${table.name}, a ${table.kindName}`}
                onClick={() => actions.openTable(connection, schema, table.name)}
                onDoubleClick={() => actions.queryTable(connection, table.preview, true)}
                onContextMenu={(event) =>
                    tree.menu(event, [
                        { label: "Open", icon: <IconFile size={13} />, run: () => actions.openTable(connection, schema, table.name) },
                        { label: "Preview rows", icon: <IconEye size={13} />, run: () => actions.queryTable(connection, table.preview, true) },
                        {
                            label: "Query this table",
                            icon: <IconCode size={13} />,
                            run: () => actions.queryTable(connection, table.preview, false),
                        },
                        { label: open ? "Hide columns" : "Show columns", icon: <IconChevron size={13} />, run: () => tree.onToggle(key) },
                        { sep: true },
                        { label: "Copy name", icon: <IconCopy size={13} />, run: () => actions.copy(table.name, "table name") },
                        {
                            label: "Copy qualified name",
                            icon: <IconCopy size={13} />,
                            run: () => actions.copy(`${schema}.${table.name}`, "qualified name"),
                        },
                    ])
                }
            />
            {open && <ColumnNodes table={table} tree={tree} />}
        </li>
    );
}

function ColumnNodes({ table, tree }: { table: DatabaseTableEntry; tree: Tree }) {
    return (
        <ul role="group">
            {table.failed ? (
                <NodeNote depth={4} error>
                    {table.failed}
                </NodeNote>
            ) : !table.columns ? (
                <NodeNote depth={4}>Loading columns…</NodeNote>
            ) : (
                table.columns.map((column) => (
                    <li key={column.name} role="treeitem" aria-label={column.name}>
                        <Row
                            depth={4}
                            icon={<span className={`db-glyph column${column.primaryKey ? " key" : ""}`}>{column.primaryKey ? "⚿" : "▫"}</span>}
                            label={column.name}
                            detail={<span className="db-node-type">{column.type}</span>}
                            title={`${column.name} ${column.type}${column.nullable ? "" : " not null"}${column.primaryKey ? ", primary key" : ""}`}
                            onContextMenu={(event) =>
                                tree.menu(event, [
                                    { label: "Copy name", icon: <IconCopy size={13} />, run: () => tree.actions.copy(column.name, "column name") },
                                ])
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
