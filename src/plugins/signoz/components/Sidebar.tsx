import { useMemo } from "react";
import { openUrl, reportError, swallow } from "../../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../../plugin-api/resources";
import { signozApi, type SignozStatus } from "../api";
import { mergeByService } from "../health";
import { signozDashboardsR, signozServicesR } from "../resources";
import { openDashboard, showSection, showService, signozSettings, updateSettings, useExploreView, type Pin, type Section } from "../state";
import { SectionIcon } from "./SectionIcon";

/** Below this, a pinned service's dot stays green: a few failures an hour are ordinary. */
const FAILING_RATE = 0.01;

const SECTIONS: { id: Section; label: string }[] = [
    { id: "services", label: "Services" },
    { id: "logs", label: "Logs" },
    { id: "traces", label: "Traces" },
    { id: "dashboards", label: "Dashboards" },
];

function Pins({ paneId, active }: { paneId: string; active: boolean }) {
    const view = useExploreView(paneId);
    const pins = signozSettings.useSelect((settings) => settings.pins);
    const minutes = signozSettings.useSelect((settings) => settings.minutes);
    const environment = signozSettings.useSelect((settings) => settings.environment);
    const wantsDashboards = pins.some((pin) => pin.startsWith("dashboard:"));
    const dashboards = useResourceEnabled(active && wantsDashboards, signozDashboardsR);
    const health = useResourceEnabled(active, signozServicesR, { minutes });
    const failing = useMemo(
        () =>
            new Set(
                mergeByService(health.data ?? [], environment)
                    .filter((row) => row.errorRate >= FAILING_RATE)
                    .map((row) => row.service),
            ),
        [environment, health.data],
    );
    if (pins.length === 0) return null;

    const row = (pin: Pin) => {
        const [type, ...rest] = pin.split(":");
        const id = rest.join(":");
        if (type === "service") {
            const on = view.section === "services" && view.service === id;
            return (
                <button key={pin} type="button" className={`sgz-nav sgz-pin${on ? " on" : ""}`} onClick={() => showService(paneId, id)}>
                    <span className={`sgz-pin-dot${failing.has(id) ? " bad" : ""}`} aria-hidden="true" />
                    <span className="sgz-nav-label mono">{id}</span>
                </button>
            );
        }
        const on = view.section === "dashboards" && view.dashboard === id;
        const title = dashboards.data?.find((dashboard) => dashboard.id === id)?.title ?? "Dashboard";
        return (
            <button key={pin} type="button" className={`sgz-nav sgz-pin${on ? " on" : ""}`} onClick={() => openDashboard(paneId, id)}>
                <SectionIcon section="dashboards" />
                <span className="sgz-nav-label">{title}</span>
            </button>
        );
    };

    return (
        <nav className="sgz-nav-group" aria-label="Pinned">
            <div className="sgz-side-head">Pinned</div>
            {pins.map(row)}
        </nav>
    );
}

export function Sidebar({ paneId, active, status }: { paneId: string; active: boolean; status: SignozStatus }) {
    const view = useExploreView(paneId);
    const minutes = signozSettings.useSelect((settings) => settings.minutes);
    const environment = signozSettings.useSelect((settings) => settings.environment);
    const health = useResourceEnabled(active, signozServicesR, { minutes });
    const environments = useMemo(
        () => [...new Set((health.data ?? []).map((row) => row.environment).filter((name): name is string => !!name))].sort(),
        [health.data],
    );
    const host = status.url.replace(/^https?:\/\//, "");
    const signOut = () =>
        void signozApi
            .signOut()
            .then(() => invalidate((kind) => kind.startsWith("signoz.")))
            .catch(reportError("sign out of SigNoz"));

    return (
        <aside className="sgz-side" aria-label="SigNoz">
            <select
                className="sgz-input sgz-side-env"
                value={environment ?? ""}
                onChange={(event) => updateSettings({ environment: event.target.value || null })}
                aria-label="Environment">
                <option value="">All environments</option>
                {environments.map((name) => (
                    <option key={name} value={name}>
                        {name}
                    </option>
                ))}
            </select>
            <nav className="sgz-nav-group" aria-label="Sections">
                {SECTIONS.map((section) => {
                    const on = view.section === section.id;
                    return (
                        <button
                            key={section.id}
                            type="button"
                            aria-current={on ? "page" : undefined}
                            className={`sgz-nav${on ? " on" : ""}`}
                            onClick={() => showSection(paneId, section.id)}>
                            <SectionIcon section={section.id} />
                            <span className="sgz-nav-label">{section.label}</span>
                        </button>
                    );
                })}
            </nav>
            <Pins paneId={paneId} active={active} />
            <footer className="sgz-side-foot">
                <div className="sgz-side-who" title={status.url}>
                    <span className="sgz-side-host">{host}</span>
                    <span className="sgz-side-account">{status.email || "API key"}</span>
                </div>
                <button type="button" className="sgz-foot-button" onClick={() => void openUrl(status.url).catch(swallow("open SigNoz"))}>
                    Open
                </button>
                <button type="button" className="sgz-foot-button" onClick={signOut}>
                    Sign out
                </button>
            </footer>
        </aside>
    );
}
