import { useMemo } from "react";
import { invalidate, useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, SkeletonRows } from "../../../plugin-api/ui";
import type { Scope } from "../api";
import { mergeByService, percent } from "../health";
import { signozDashboardsR, signozServicesR, signozStatusR } from "../resources";
import {
    WINDOWS,
    scopeOf,
    setLive,
    showSection,
    signalOf,
    signozSettings,
    togglePin,
    updateSettings,
    updateView,
    useExploreView,
    type ExploreView,
    type Pin,
    type ServiceTab,
} from "../state";
import { DashboardList } from "./DashboardList";
import { DashboardView } from "./DashboardView";
import { FilterBar } from "./FilterBar";
import { LogFeed } from "./LogFeed";
import { ServiceOverview } from "./ServiceOverview";
import { ServicesTable } from "./ServicesTable";
import { Sidebar } from "./Sidebar";
import { SignozSignIn } from "./SignozSignIn";
import { TraceList } from "./TraceList";
import { TraceView } from "./TraceView";
import { timeLabel } from "./charts";
import "../signoz.css";

const refreshAll = () => invalidate((kind) => kind.startsWith("signoz."));

export function windowLabel(minutes: number): string {
    if (minutes >= 1440) return `${minutes / 1440}d`;
    return minutes >= 60 ? `${minutes / 60}h` : `${minutes}m`;
}

const SERVICE_TABS: { id: ServiceTab; label: string }[] = [
    { id: "overview", label: "Overview" },
    { id: "logs", label: "Logs" },
    { id: "traces", label: "Traces" },
];

const SECTION_TITLES = { services: "Services", logs: "Logs", traces: "Traces", dashboards: "Dashboards" } as const;

function TimeControls({ paneId }: { paneId: string }) {
    const view = useExploreView(paneId);
    const minutes = signozSettings.useSelect((settings) => settings.minutes);
    const pausedAt = view.range ?? (view.fixedEnd ? { start: view.fixedEnd - minutes * 60_000, end: view.fixedEnd } : null);
    return (
        <div className="sgz-time">
            <div className="sgz-segmented" role="group" aria-label="Time window">
                {WINDOWS.map((option) => (
                    <button
                        key={option}
                        type="button"
                        aria-pressed={!view.range && minutes === option}
                        className={!view.range && minutes === option ? "on" : ""}
                        onClick={() => {
                            updateSettings({ minutes: option });
                            if (view.range) setLive(paneId, view.live);
                        }}>
                        {windowLabel(option)}
                    </button>
                ))}
            </div>
            <button
                type="button"
                className={`sgz-live${view.live ? " on" : ""}`}
                aria-pressed={view.live}
                onClick={() => setLive(paneId, !view.live)}
                title={view.live ? "Following new data. Click to hold still." : "Held still. Click to follow new data."}>
                <span className="sgz-live-dot" aria-hidden="true" />
                {view.live
                    ? "Live"
                    : pausedAt
                      ? `${timeLabel(pausedAt.start, pausedAt.end - pausedAt.start)} – ${timeLabel(pausedAt.end, pausedAt.end - pausedAt.start)}`
                      : "Paused"}
            </button>
        </div>
    );
}

function PinButton({ pin }: { pin: Pin }) {
    const pinned = signozSettings.useSelect((settings) => settings.pins.includes(pin));
    return (
        <button
            type="button"
            className={`sgz-pin-button${pinned ? " on" : ""}`}
            aria-pressed={pinned}
            onClick={() => togglePin(pin)}
            title={pinned ? "Unpin from the sidebar" : "Pin to the sidebar"}>
            {pinned ? "Pinned" : "Pin"}
        </button>
    );
}

function ServiceBadge({ service, active }: { service: string; active: boolean }) {
    const minutes = signozSettings.useSelect((settings) => settings.minutes);
    const environment = signozSettings.useSelect((settings) => settings.environment);
    const health = useResourceEnabled(active, signozServicesR, { minutes });
    const row = useMemo(
        () => mergeByService(health.data ?? [], environment).find((candidate) => candidate.service === service),
        [environment, health.data, service],
    );
    if (!row || row.errors === 0) return null;
    return <span className="sgz-badge bad">{percent(row.errorRate)} errors</span>;
}

function Title({ paneId, view, active }: { paneId: string; view: ExploreView; active: boolean }) {
    const dashboards = useResourceEnabled(active && !!view.dashboard, signozDashboardsR);
    const root = SECTION_TITLES[view.section];
    const entity =
        view.section === "services" && view.service
            ? { name: view.service, mono: true, pin: `service:${view.service}` as Pin }
            : view.section === "dashboards" && view.dashboard
              ? {
                    name: dashboards.data?.find((dashboard) => dashboard.id === view.dashboard)?.title ?? "Dashboard",
                    mono: false,
                    pin: `dashboard:${view.dashboard}` as Pin,
                }
              : null;
    if (!entity) {
        return (
            <div className="sgz-crumbs">
                <h2 className="sgz-crumb-current">{root}</h2>
            </div>
        );
    }
    return (
        <div className="sgz-crumbs">
            <button type="button" className="sgz-crumb" onClick={() => showSection(paneId, view.section)}>
                {root}
            </button>
            <span className="sgz-crumb-sep" aria-hidden="true">
                /
            </span>
            <h2 className={`sgz-crumb-current${entity.mono ? " mono" : ""}`}>{entity.name}</h2>
            {view.section === "services" && view.service && <ServiceBadge service={view.service} active={active} />}
            <PinButton pin={entity.pin} />
        </div>
    );
}

function Body({ paneId, view, scope, active, signozUrl }: { paneId: string; view: ExploreView; scope: Scope; active: boolean; signozUrl: string }) {
    if (view.trace) return <TraceView traceId={view.trace} onBack={() => updateView(paneId, { trace: null })} />;
    const signal = signalOf(view);
    if (signal) {
        return (
            <>
                <FilterBar paneId={paneId} signal={signal} />
                {signal === "logs" ? <LogFeed paneId={paneId} active={active} /> : <TraceList paneId={paneId} active={active} />}
            </>
        );
    }
    if (view.section === "dashboards") {
        return view.dashboard ? (
            <DashboardView dashboardId={view.dashboard} scope={scope} active={active} signozUrl={signozUrl} />
        ) : (
            <DashboardList paneId={paneId} active={active} />
        );
    }
    return view.service ? (
        <ServiceOverview paneId={paneId} service={view.service} scope={scope} active={active} />
    ) : (
        <ServicesTable paneId={paneId} active={active} />
    );
}

export function SignozPane({ paneId, active }: { paneId: string; active: boolean }) {
    const status = useResourceEnabled(active, signozStatusR);
    const view = useExploreView(paneId);
    const minutes = signozSettings.useSelect((settings) => settings.minutes);
    const environment = signozSettings.useSelect((settings) => settings.environment);
    const scope = useMemo(() => scopeOf(view, { minutes, environment }), [view, minutes, environment]);

    if (!status.data) {
        return (
            <div className="sgz-pane">
                {status.error ? <EmptyState tone="error" message={String(status.error)} /> : <SkeletonRows rows={6} label="Connecting to SigNoz" />}
            </div>
        );
    }
    if (!status.data.ok) {
        return (
            <div className="sgz-pane">
                <SignozSignIn status={status.data} onSignedIn={refreshAll} />
            </div>
        );
    }

    const servicePage = view.section === "services" && !!view.service;
    return (
        <div className="sgz-pane sgz-layout">
            <Sidebar paneId={paneId} active={active} status={status.data} />
            <section className="sgz-main">
                <header className="sgz-head">
                    <Title paneId={paneId} view={view} active={active} />
                    <TimeControls paneId={paneId} />
                </header>
                {servicePage && (
                    <nav className="sgz-subtabs" role="tablist" aria-label="Service views">
                        {SERVICE_TABS.map((tab) => {
                            const on = view.serviceTab === tab.id && !view.trace;
                            return (
                                <button
                                    key={tab.id}
                                    type="button"
                                    role="tab"
                                    aria-selected={on}
                                    className={`sgz-subtab${on ? " on" : ""}`}
                                    onClick={() => updateView(paneId, { serviceTab: tab.id, trace: null })}>
                                    {tab.label}
                                </button>
                            );
                        })}
                    </nav>
                )}
                <Body paneId={paneId} view={view} scope={scope} active={active} signozUrl={status.data.url} />
            </section>
        </div>
    );
}
