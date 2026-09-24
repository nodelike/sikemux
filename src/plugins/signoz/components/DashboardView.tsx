import { useMemo } from "react";
import { openUrl, swallow } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, SkeletonRows } from "../../../plugin-api/ui";
import { failureMessage, type DashboardPanel, type PanelData, type Scope } from "../api";
import { signozDashboardR, signozPanelR } from "../resources";
import { setDashboardVariable, signozSettings } from "../state";
import { BarList, DataTable, StatValue, TimeChart } from "./charts";

/** SigNoz lays dashboards out on twelve columns; a row unit is about this tall. */
const ROW_HEIGHT = 44;

function pieRows(data: Extract<PanelData, { shape: "table" }>) {
    const value = data.columns.map((column) => column.aggregation).lastIndexOf(true);
    return data.rows
        .map((row) => ({
            label:
                row
                    .filter((_, index) => !data.columns[index]?.aggregation)
                    .map(String)
                    .join(" · ") || "value",
            value: Number(row[value] ?? 0),
        }))
        .sort((left, right) => right.value - left.value);
}

function PanelBody({ panel, data }: { panel: DashboardPanel; data: PanelData }) {
    if (data.shape === "value") return <StatValue value={data.value} unit={panel.unit} />;
    if (data.shape === "series") {
        if (data.series.length === 0) return <EmptyState message="No data in this window." />;
        return <TimeChart series={data.series} unit={panel.unit} bars={panel.kind === "bar"} />;
    }
    if (data.rows.length === 0) return <EmptyState message="No rows in this window." />;
    if (panel.kind === "pie") return <BarList rows={pieRows(data)} unit={panel.unit} />;
    return <DataTable columns={data.columns} rows={data.rows} unit={panel.unit} />;
}

/** A panel one row tall has no room for a chart; dashboards use them as section headings. */
export function isHeading(panel: DashboardPanel): boolean {
    return panel.layout.h <= 1;
}

/** Where SigNoz saved the panel, on its twelve-column grid, at exactly the height it was given. */
export function placement(panel: DashboardPanel): React.CSSProperties {
    const x = Math.min(Math.max(0, panel.layout.x), 11);
    const width = Math.max(1, Math.min(panel.layout.w, 12 - x));
    const height = Math.max(1, panel.layout.h);
    return {
        gridColumn: `${x + 1} / span ${width}`,
        gridRow: panel.layout.y < 10_000 ? `${panel.layout.y + 1} / span ${height}` : `span ${height}`,
    };
}

function PanelCard({
    panel,
    scope,
    variables,
    active,
    openInSignoz,
}: {
    panel: DashboardPanel;
    scope: Scope;
    variables: Record<string, string>;
    active: boolean;
    openInSignoz: () => void;
}) {
    const heading = isHeading(panel);
    const data = useResourceEnabled(active && panel.drawable && !heading, signozPanelR, {
        ...scope,
        kind: panel.kind,
        query: panel.query,
        variables,
    });
    const place = placement(panel);
    if (heading) {
        return (
            <h3 className="sgz-section" style={place}>
                {panel.title}
            </h3>
        );
    }
    return (
        <section className={`sgz-panel kind-${panel.kind}`} style={place} aria-label={panel.title}>
            <header className="sgz-panel-head">
                <h3>{panel.title || "Untitled panel"}</h3>
            </header>
            <div className="sgz-panel-body">
                {!panel.drawable ? (
                    <EmptyState
                        message={`Sikemux does not draw ${panel.kind} panels yet.`}
                        action={{ label: "Open in SigNoz", onClick: openInSignoz }}
                    />
                ) : data.error ? (
                    <EmptyState tone="error" message={failureMessage(data.error)} />
                ) : !data.data ? (
                    <SkeletonRows rows={3} label={`Loading ${panel.title}`} />
                ) : (
                    <PanelBody panel={panel} data={data.data} />
                )}
            </div>
        </section>
    );
}

export function DashboardView({ dashboardId, scope, active, signozUrl }: { dashboardId: string; scope: Scope; active: boolean; signozUrl: string }) {
    const dashboard = useResourceEnabled(active, signozDashboardR, dashboardId);
    const chosen = signozSettings.useSelect((settings) => settings.dashboardVariables[dashboardId]);
    const variables = useMemo(
        () => Object.fromEntries((dashboard.data?.variables ?? []).map((variable) => [variable.name, chosen?.[variable.name] ?? variable.selected])),
        [chosen, dashboard.data],
    );
    const openInSignoz = () => void openUrl(`${signozUrl}/dashboard/${dashboardId}`).catch(swallow("open the dashboard in SigNoz"));

    if (dashboard.error) return <EmptyState tone="error" message={failureMessage(dashboard.error)} />;
    if (!dashboard.data) return <SkeletonRows rows={8} label="Loading dashboard" />;

    return (
        <div className="sgz-dashboard">
            {dashboard.data.variables.length > 0 && (
                <div className="sgz-variables" role="group" aria-label="Dashboard variables">
                    {dashboard.data.variables.map((variable) => (
                        <label key={variable.name} className="sgz-variable">
                            <span>{variable.name}</span>
                            <select
                                className="sgz-input"
                                value={variables[variable.name]}
                                onChange={(event) => setDashboardVariable(dashboardId, variable.name, event.target.value)}>
                                {[...new Set([variables[variable.name], ...variable.options])].map((option) => (
                                    <option key={option} value={option}>
                                        {option}
                                    </option>
                                ))}
                            </select>
                        </label>
                    ))}
                    <button type="button" className="sgz-link sgz-variables-open" onClick={openInSignoz}>
                        Open in SigNoz
                    </button>
                </div>
            )}
            <div className="sgz-grid" style={{ gridAutoRows: `${ROW_HEIGHT}px` }}>
                {dashboard.data.panels.map((panel) => (
                    <PanelCard key={panel.id} panel={panel} scope={scope} variables={variables} active={active} openInSignoz={openInSignoz} />
                ))}
            </div>
        </div>
    );
}
