import { useMemo, useState } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, IconSearch, SkeletonRows, rankBy } from "../../../plugin-api/ui";
import { failureMessage } from "../api";
import { SORTERS, mergeByService, perMinute, percent } from "../health";
import { signozServicesR } from "../resources";
import { showService, signozSettings, updateSettings, type ServiceSort } from "../state";
import { formatValue } from "./charts";

const COLUMNS: { sort: ServiceSort; label: string; numeric: boolean }[] = [
    { sort: "name", label: "Service", numeric: false },
    { sort: "calls", label: "Requests", numeric: true },
    { sort: "errors", label: "Error rate", numeric: true },
    { sort: "p99", label: "p99", numeric: true },
];

export function ServicesTable({ paneId, active }: { paneId: string; active: boolean }) {
    const minutes = signozSettings.useSelect((settings) => settings.minutes);
    const environment = signozSettings.useSelect((settings) => settings.environment);
    const sort = signozSettings.useSelect((settings) => settings.serviceSort);
    const health = useResourceEnabled(active, signozServicesR, { minutes });
    const [query, setQuery] = useState("");

    const rows = useMemo(() => {
        const merged = mergeByService(health.data ?? [], environment).sort(SORTERS[sort]);
        return query.trim() ? rankBy(query.trim(), merged, (row) => row.service) : merged;
    }, [environment, health.data, query, sort]);

    return (
        <div className="sgz-page">
            <div className="sgz-page-bar">
                <label className="sgz-search">
                    <IconSearch size={12} />
                    <input
                        className="sgz-input"
                        placeholder="Find a service"
                        value={query}
                        onChange={(event) => setQuery(event.target.value)}
                        onKeyDown={(event) => {
                            if (event.key === "Enter" && rows[0]) showService(paneId, rows[0].service);
                        }}
                        spellCheck={false}
                        aria-label="Find a service"
                    />
                </label>
                {health.data && (
                    <span className="sgz-muted">
                        {rows.length} {rows.length === 1 ? "service" : "services"}
                    </span>
                )}
            </div>
            {health.error && <EmptyState tone="error" message={failureMessage(health.error)} />}
            {!health.data && !health.error && <SkeletonRows rows={8} label="Reading services" />}
            {health.data && rows.length === 0 && <EmptyState message={query ? "No service matches." : "No traced services in this window."} />}
            {rows.length > 0 && (
                <div className="sgz-list-table-scroll">
                    <table className="sgz-list-table sgz-services">
                        <thead>
                            <tr>
                                {COLUMNS.map((column) => (
                                    <th
                                        key={column.sort}
                                        className={column.numeric ? "num" : ""}
                                        aria-sort={sort === column.sort ? (column.sort === "name" ? "ascending" : "descending") : undefined}>
                                        <button
                                            type="button"
                                            className={`sgz-th${sort === column.sort ? " on" : ""}`}
                                            onClick={() => updateSettings({ serviceSort: column.sort })}>
                                            {column.label}
                                        </button>
                                    </th>
                                ))}
                            </tr>
                        </thead>
                        <tbody>
                            {rows.map((row) => (
                                <tr key={row.service} className="sgz-row-link" onClick={() => showService(paneId, row.service)}>
                                    <td>
                                        <button type="button" className="sgz-cell-link">
                                            {row.service}
                                        </button>
                                    </td>
                                    <td className="num">{perMinute(row.calls, minutes)}</td>
                                    <td className={`num${row.errors > 0 ? " bad" : " quiet"}`}>{percent(row.errorRate)}</td>
                                    <td className="num">{formatValue(row.p99Ms, "ms")}</td>
                                </tr>
                            ))}
                        </tbody>
                    </table>
                </div>
            )}
        </div>
    );
}
