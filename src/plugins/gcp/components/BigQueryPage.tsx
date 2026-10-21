import { errorText } from "../../../plugin-api/format";
import { useState } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, SkeletonRows } from "../../../plugin-api/ui";
import { gcpApi, type BqField, type BqResult, type Target } from "../api";
import { datasetsR, tableR, tablesR } from "../resources";
import { updateGcpView, useGcpView } from "../state";
import type { ProductProps } from "./GcpPane";
import { Button, consoleUrl, Facts, keyOf, PageHeader, size, Tabs, visit } from "./parts";
import { Problem } from "./Problem";

const cell = (value: unknown) => (value === null || value === undefined ? "null" : typeof value === "object" ? JSON.stringify(value) : String(value));

/** Rows as a grid with a column per field, as BigQuery's results show them. */
function Rows({ schema, rows }: { schema: BqField[]; rows: Record<string, unknown>[] }) {
    if (rows.length === 0) return <EmptyState message="No rows." />;
    return (
        <div className="gcp-grid-wrap">
            <table className="gcp-grid">
                <thead>
                    <tr>
                        <th>Row</th>
                        {schema.map((field) => (
                            <th key={field.name}>{field.name}</th>
                        ))}
                    </tr>
                </thead>
                <tbody>
                    {rows.map((row, index) => (
                        <tr key={index}>
                            <td className="gcp-muted">{index + 1}</td>
                            {schema.map((field) => (
                                <td key={field.name}>{cell(row[field.name])}</td>
                            ))}
                        </tr>
                    ))}
                </tbody>
            </table>
        </div>
    );
}

function Schema({ fields, depth = 0 }: { fields: BqField[]; depth?: number }) {
    return (
        <>
            {fields.map((field) => (
                <div key={`${depth}-${field.name}`}>
                    <div className="gcp-schema-row" style={{ paddingLeft: 12 + depth * 18 }}>
                        <span className="gcp-mono">{field.name}</span>
                        <span>{field.type}</span>
                        <span className="gcp-muted">{field.mode}</span>
                        <span className="gcp-muted">{field.description}</span>
                    </div>
                    {field.fields.length > 0 && <Schema fields={field.fields} depth={depth + 1} />}
                </div>
            ))}
        </>
    );
}

export function BigQueryPage({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    const [sql, setSql] = useState("");
    const opened = view.open?.split(".") ?? null;
    return (
        <div className="gcp-page">
            <PageHeader title="BigQuery">
                <Button onClick={() => visit(consoleUrl("bigquery", target.project))}>Open BigQuery Studio</Button>
            </PageHeader>
            <div className="gcp-studio">
                <Explorer
                    active={active}
                    target={target}
                    selected={view.open}
                    onTable={(dataset, table) => {
                        updateGcpView(paneId, { open: `${dataset}.${table}`, tab: null });
                        if (!sql.trim()) setSql(`SELECT *\nFROM \`${target.project}.${dataset}.${table}\`\nLIMIT 100`);
                    }}
                />
                <div className="gcp-studio-main">
                    <QueryEditor target={target} sql={sql} onSql={setSql} />
                    {opened && opened.length === 2 && (
                        <TableView paneId={paneId} active={active} target={target} dataset={opened[0]} table={opened[1]} />
                    )}
                </div>
            </div>
        </div>
    );
}

function Explorer({
    active,
    target,
    selected,
    onTable,
}: {
    active: boolean;
    target: Target;
    selected: string | null;
    onTable: (dataset: string, table: string) => void;
}) {
    const datasets = useResourceEnabled(active, datasetsR, ...keyOf(target));
    const [open, setOpen] = useState<string | null>(null);
    return (
        <nav className="gcp-explorer" aria-label="Datasets">
            <div className="gcp-explorer-head">{target.project}</div>
            {datasets.status === "error" && !datasets.data ? (
                <p className="gcp-note">{datasets.error}</p>
            ) : !datasets.data ? (
                <SkeletonRows rows={4} label="Loading datasets" />
            ) : datasets.data.length === 0 ? (
                <p className="gcp-note">No datasets.</p>
            ) : (
                datasets.data.map((dataset) => (
                    <div key={dataset.id}>
                        <button
                            type="button"
                            className="gcp-explorer-item"
                            aria-expanded={open === dataset.id}
                            onClick={() => setOpen(open === dataset.id ? null : dataset.id)}>
                            <span className="gcp-muted">{open === dataset.id ? "▾" : "▸"}</span> {dataset.id}
                        </button>
                        {open === dataset.id && <Tables active={active} target={target} dataset={dataset.id} selected={selected} onTable={onTable} />}
                    </div>
                ))
            )}
        </nav>
    );
}

function Tables({
    active,
    target,
    dataset,
    selected,
    onTable,
}: {
    active: boolean;
    target: Target;
    dataset: string;
    selected: string | null;
    onTable: (dataset: string, table: string) => void;
}) {
    const tables = useResourceEnabled(active, tablesR, ...keyOf(target), dataset);
    if (!tables.data) return <p className="gcp-note gcp-explorer-nested">{tables.status === "error" ? tables.error : "Loading…"}</p>;
    if (tables.data.length === 0) return <p className="gcp-note gcp-explorer-nested">No tables.</p>;
    return (
        <>
            {tables.data.map((table) => (
                <button
                    key={table.id}
                    type="button"
                    className={`gcp-explorer-item nested${selected === `${dataset}.${table.id}` ? " on" : ""}`}
                    onClick={() => onTable(dataset, table.id)}>
                    {table.type === "VIEW" ? "◇" : "▦"} {table.id}
                </button>
            ))}
        </>
    );
}

function QueryEditor({ target, sql, onSql }: { target: Target; sql: string; onSql: (sql: string) => void }) {
    const [result, setResult] = useState<BqResult | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [busy, setBusy] = useState<"estimate" | "run" | null>(null);
    const go = (dryRun: boolean) => {
        setBusy(dryRun ? "estimate" : "run");
        setError(null);
        gcpApi
            .query(target, sql, dryRun)
            .then(setResult)
            .catch((failure: unknown) => setError(errorText(failure)))
            .finally(() => setBusy(null));
    };
    return (
        <section className="gcp-section" aria-label="Query">
            <textarea
                className="gcp-input gcp-sql"
                value={sql}
                onChange={(event) => onSql(event.target.value)}
                onKeyDown={(event) => {
                    if (event.key === "Enter" && (event.metaKey || event.ctrlKey) && sql.trim()) go(false);
                }}
                placeholder="SELECT … in standard SQL. ⌘↵ runs it."
                aria-label="SQL"
                spellCheck={false}
            />
            <div className="gcp-log-bar">
                <Button primary disabled={!sql.trim() || busy !== null} onClick={() => go(false)}>
                    {busy === "run" ? "Running…" : "Run"}
                </Button>
                <Button disabled={!sql.trim() || busy !== null} onClick={() => go(true)}>
                    {busy === "estimate" ? "Estimating…" : "Estimate"}
                </Button>
                <span className="gcp-muted">Runs bill at most 10 GiB; a bigger query is refused before it starts.</span>
            </div>
            {error && <p className="gcp-error-text">{error}</p>}
            {result &&
                (result.dryRun ? (
                    <p className="gcp-note">
                        This query will process <b>{size(result.bytes)}</b> when run, about <b>${result.costUsd.toFixed(4)}</b> on demand.
                    </p>
                ) : (
                    <>
                        <p className="gcp-note">
                            {result.totalRows ?? result.rows.length} rows · {size(result.bytes)} billed{result.cached ? " (cached)" : ""} · about $
                            {result.costUsd.toFixed(4)}
                        </p>
                        <Rows schema={result.schema} rows={result.rows} />
                    </>
                ))}
        </section>
    );
}

const TABLE_TABS = [
    { value: "schema", label: "Schema" },
    { value: "details", label: "Details" },
    { value: "preview", label: "Preview" },
] as const;

function TableView({ paneId, active, target, dataset, table }: { paneId: string; active: boolean; target: Target; dataset: string; table: string }) {
    const view = useGcpView(paneId);
    const tab = TABLE_TABS.find((each) => each.value === view.tab)?.value ?? "schema";
    const found = useResourceEnabled(active, tableR, ...keyOf(target), dataset, table);
    return (
        <section className="gcp-section" aria-label={`Table ${table}`}>
            <h2 className="gcp-section-title">
                {dataset}.{table}
            </h2>
            <Tabs label={table} tabs={TABLE_TABS} value={tab} onChange={(value) => updateGcpView(paneId, { tab: value })} />
            {found.status === "error" && !found.data ? (
                <Problem error={found.error} what="the table" onRetry={() => void found.refresh()} />
            ) : !found.data ? (
                <SkeletonRows rows={5} label="Loading the table" />
            ) : tab === "schema" ? (
                <div className="gcp-schema">
                    <div className="gcp-schema-row head">
                        <span>Field name</span>
                        <span>Type</span>
                        <span>Mode</span>
                        <span>Description</span>
                    </div>
                    <Schema fields={found.data.schema} />
                </div>
            ) : tab === "details" ? (
                <>
                    <Facts
                        items={[
                            ["Type", found.data.type.toLowerCase()],
                            ["Rows", found.data.rows?.toLocaleString()],
                            ["Size", size(found.data.bytes)],
                            ["Location", found.data.location],
                            ["Partitioned by", found.data.partitionedBy],
                            ["Clustered by", found.data.clusteredBy?.join(", ")],
                            ["Last modified", found.data.modifiedAt ? new Date(found.data.modifiedAt).toLocaleString() : null],
                            ["Description", found.data.description],
                        ]}
                    />
                    {found.data.view && <pre className="gcp-terminal">{found.data.view}</pre>}
                </>
            ) : found.data.preview ? (
                <Rows schema={found.data.schema} rows={found.data.preview} />
            ) : (
                <EmptyState message="Only tables can be previewed for free; query a view instead." />
            )}
        </section>
    );
}
