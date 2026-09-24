import { useMemo, useState } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, IconSearch, SkeletonRows, rankBy } from "../../../plugin-api/ui";
import { failureMessage } from "../api";
import { signozDashboardsR } from "../resources";
import { openDashboard } from "../state";

export function DashboardList({ paneId, active }: { paneId: string; active: boolean }) {
    const dashboards = useResourceEnabled(active, signozDashboardsR);
    const [query, setQuery] = useState("");
    const shown = useMemo(() => {
        const all = [...(dashboards.data ?? [])].sort((left, right) => left.title.localeCompare(right.title));
        return query.trim() ? rankBy(query.trim(), all, (dashboard) => `${dashboard.title} ${dashboard.tags.join(" ")}`) : all;
    }, [dashboards.data, query]);

    return (
        <div className="sgz-page">
            <div className="sgz-page-bar">
                <label className="sgz-search">
                    <IconSearch size={12} />
                    <input
                        className="sgz-input"
                        placeholder="Find a dashboard"
                        value={query}
                        onChange={(event) => setQuery(event.target.value)}
                        onKeyDown={(event) => {
                            if (event.key === "Enter" && shown[0]) openDashboard(paneId, shown[0].id);
                        }}
                        spellCheck={false}
                        aria-label="Find a dashboard"
                    />
                </label>
                {dashboards.data && <span className="sgz-muted">{shown.length} dashboards</span>}
            </div>
            {dashboards.error && <EmptyState tone="error" message={failureMessage(dashboards.error)} />}
            {!dashboards.data && !dashboards.error && <SkeletonRows rows={6} label="Reading dashboards" />}
            {dashboards.data && shown.length === 0 && <EmptyState message={query ? "No dashboard matches." : "No dashboards yet."} />}
            {shown.length > 0 && (
                <div className="sgz-list-table-scroll">
                    <table className="sgz-list-table">
                        <thead>
                            <tr>
                                <th>Dashboard</th>
                                <th>Tags</th>
                                <th className="num">Panels</th>
                            </tr>
                        </thead>
                        <tbody>
                            {shown.map((dashboard) => (
                                <tr key={dashboard.id} className="sgz-row-link" onClick={() => openDashboard(paneId, dashboard.id)}>
                                    <td>
                                        <button type="button" className="sgz-cell-link sgz-cell-ui">
                                            {dashboard.title}
                                        </button>
                                        {dashboard.description && <div className="sgz-cell-note">{dashboard.description}</div>}
                                    </td>
                                    <td>
                                        <span className="sgz-tags">
                                            {dashboard.tags.map((tag) => (
                                                <span key={tag} className="sgz-tag">
                                                    {tag}
                                                </span>
                                            ))}
                                        </span>
                                    </td>
                                    <td className="num">{dashboard.panels}</td>
                                </tr>
                            ))}
                        </tbody>
                    </table>
                </div>
            )}
        </div>
    );
}
