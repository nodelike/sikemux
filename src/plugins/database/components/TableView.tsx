import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, SkeletonRows } from "../../../plugin-api/ui";
import { databaseTableV } from "../resources";
import type { OpenTable } from "../tabs";
import type { DatabaseConnection } from "../types.generated";

export function TableView({
    slot,
    profile,
    table,
    active,
    onQuery,
    onOpenTable,
}: {
    /** Where the pane shows it, so the core keeps it for this pane. */
    slot: string;
    profile: DatabaseConnection;
    table: OpenTable;
    active: boolean;
    /** Puts SQL in the editor, running it when asked. */
    onQuery: (sql: string, run: boolean) => void;
    onOpenTable: (table: OpenTable) => void;
}) {
    const found = useResourceEnabled(active, databaseTableV, slot, profile.id, table.schema, table.name);
    if (found.status === "error") return <EmptyState message={found.error ?? `Sikemux could not read ${table.name}.`} tone="error" />;
    if (!found.data) return <SkeletonRows rows={6} label={`Loading ${table.name}`} />;
    const info = found.data;

    return (
        <article className="db-table-view" aria-label={`${info.schema}.${info.name}`}>
            <header className="db-detail-head">
                <h2>
                    <span className="db-meta">{info.schema}.</span>
                    {info.name}
                </h2>
                {info.kind !== "table" && <span className="db-badge">{info.kindName}</span>}
                <span className="db-grow" />
                <button type="button" className="db-button" onClick={() => onQuery(info.preview, false)}>
                    Query this table
                </button>
                <button type="button" className="db-button primary" onClick={() => onQuery(info.preview, true)}>
                    Preview rows
                </button>
            </header>
            <section aria-label="Columns">
                <div className="db-heading">Columns</div>
                <table className="db-structure">
                    <thead>
                        <tr>
                            <th>Name</th>
                            <th>Type</th>
                            <th>Null</th>
                            <th>Default</th>
                        </tr>
                    </thead>
                    <tbody>
                        {info.columns.map((column) => (
                            <tr key={column.name}>
                                <td className="mono">
                                    {column.name}
                                    {column.primaryKey && (
                                        <span className="db-key" title="Primary key">
                                            PK
                                        </span>
                                    )}
                                </td>
                                <td className="mono">{column.type}</td>
                                <td>{column.nullable ? "yes" : "no"}</td>
                                <td className="mono db-meta">{column.default ?? ""}</td>
                            </tr>
                        ))}
                    </tbody>
                </table>
            </section>
            {info.indexes.length > 0 && (
                <section aria-label="Indexes">
                    <div className="db-heading">Indexes</div>
                    <ul className="db-facts-list">
                        {info.indexes.map((index) => (
                            <li key={index.name}>
                                <span className="mono">{index.name}</span>
                                <span className="db-meta">{index.detail}</span>
                            </li>
                        ))}
                    </ul>
                </section>
            )}
            {info.foreignKeys.length > 0 && (
                <section aria-label="Foreign keys">
                    <div className="db-heading">Foreign keys</div>
                    <ul className="db-facts-list">
                        {info.foreignKeys.map((key, at) => (
                            <li key={at}>
                                <span className="mono">{key.columns}</span>
                                <span className="db-meta">→</span>
                                <button
                                    type="button"
                                    className="db-link mono"
                                    title={`Open ${key.schema}.${key.table}`}
                                    onClick={() => onOpenTable({ schema: key.schema, name: key.table })}>
                                    {key.target}
                                </button>
                            </li>
                        ))}
                    </ul>
                </section>
            )}
        </article>
    );
}
