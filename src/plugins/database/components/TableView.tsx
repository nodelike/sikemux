import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, SkeletonRows } from "../../../plugin-api/ui";
import type { DatabaseProfile } from "../api";
import { databaseTableR } from "../resources";
import { previewSql } from "../sql";
import type { OpenTable } from "../tabs";

export function TableView({
    profile,
    table,
    active,
    onQuery,
    onOpenTable,
}: {
    profile: DatabaseProfile;
    table: OpenTable;
    active: boolean;
    /** Puts SQL in the editor, running it when asked. */
    onQuery: (sql: string, run: boolean) => void;
    onOpenTable: (table: OpenTable) => void;
}) {
    const found = useResourceEnabled(active, databaseTableR, profile.id, table.schema, table.name);
    if (found.status === "error") return <EmptyState message={found.error ?? `Sikemux could not read ${table.name}.`} tone="error" />;
    if (!found.data) return <SkeletonRows rows={6} label={`Loading ${table.name}`} />;
    const info = found.data;
    const preview = previewSql(profile.engine, profile.engine === "sqlite" && info.schema === "main" ? "" : info.schema, info.name);

    return (
        <article className="db-table-view" aria-label={`${info.schema}.${info.name}`}>
            <header className="db-detail-head">
                <h2>
                    <span className="db-meta">{info.schema}.</span>
                    {info.name}
                </h2>
                {info.kind !== "table" && <span className="db-badge">{info.kind.replace("-", " ")}</span>}
                <span className="db-grow" />
                <button type="button" className="db-button" onClick={() => onQuery(preview, false)}>
                    Query this table
                </button>
                <button type="button" className="db-button primary" onClick={() => onQuery(preview, true)}>
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
                                <span className="db-meta">
                                    ({index.columns.join(", ")}){index.primary ? " primary" : index.unique ? " unique" : ""}
                                </span>
                            </li>
                        ))}
                    </ul>
                </section>
            )}
            {info.foreignKeys.length > 0 && (
                <section aria-label="Foreign keys">
                    <div className="db-heading">Foreign keys</div>
                    <ul className="db-facts-list">
                        {info.foreignKeys.map((key, at) => {
                            const schema = key.referencesSchema ?? info.schema;
                            return (
                                <li key={key.name ?? at}>
                                    <span className="mono">{key.columns.join(", ")}</span>
                                    <span className="db-meta">→</span>
                                    <button
                                        type="button"
                                        className="db-link mono"
                                        title={`Open ${schema}.${key.referencesTable}`}
                                        onClick={() => onOpenTable({ schema, name: key.referencesTable })}>
                                        {key.referencesTable}({key.referencesColumns.join(", ")})
                                    </button>
                                </li>
                            );
                        })}
                    </ul>
                </section>
            )}
        </article>
    );
}
