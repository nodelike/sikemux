import { useEffect, useMemo, useRef, useState } from "react";
import { useActiveSurfacePane } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { IconSearch, rankBy, useMouseActive } from "../../../plugin-api/ui";
import { SIGNOZ_EXPLORE } from "../kinds";
import { signozDashboardsR, signozServicesR } from "../resources";
import { closePalette, openDashboard, setLive, showSection, showService, signozSettings, updateView, viewOf } from "../state";
import { mergeByService } from "../health";
import { SignozIcon } from "./SignozIcon";

const TRACE_ID = /^[0-9a-f]{16,32}$/i;

interface Item {
    id: string;
    label: string;
    hint?: string;
    run: (paneId: string) => void;
}

/** Everything the palette can do for a pane, before the query narrows it. */
export function paletteItems(query: string, services: readonly string[], dashboards: readonly { id: string; title: string }[] = []): Item[] {
    const typed = query.trim();
    const actions: Item[] = [
        { id: "section:services", label: "All services", run: (paneId) => showSection(paneId, "services") },
        { id: "section:logs", label: "Search logs", run: (paneId) => showSection(paneId, "logs") },
        { id: "section:traces", label: "Search traces", run: (paneId) => showSection(paneId, "traces") },
        { id: "section:dashboards", label: "All dashboards", run: (paneId) => showSection(paneId, "dashboards") },
        { id: "live", label: "Follow live / pause", run: (paneId) => setLive(paneId, !viewOf(paneId).live) },
        { id: "clear", label: "Clear filters", run: (paneId) => updateView(paneId, { filters: [], expression: "", text: "" }) },
    ];
    const serviceItems: Item[] = services.map((service) => ({
        id: `service:${service}`,
        label: service,
        hint: "service",
        run: (paneId) => showService(paneId, service),
    }));
    const dashboardItems: Item[] = dashboards.map((dashboard) => ({
        id: `dashboard:${dashboard.id}`,
        label: dashboard.title,
        hint: "dashboard",
        run: (paneId) => openDashboard(paneId, dashboard.id),
    }));
    const everything = [...dashboardItems, ...serviceItems];
    const ranked = typed ? rankBy(typed, [...everything, ...actions], (item) => item.label) : [...actions, ...everything];
    if (!TRACE_ID.test(typed)) return ranked;
    const trace: Item = {
        id: `trace:${typed}`,
        label: `Open trace ${typed.toLowerCase()}`,
        hint: "trace",
        run: (paneId) => updateView(paneId, { trace: typed.toLowerCase() }),
    };
    return [trace, ...ranked];
}

export function Palette() {
    const paneId = useActiveSurfacePane(SIGNOZ_EXPLORE);
    const minutes = signozSettings.useSelect((settings) => settings.minutes);
    const environment = signozSettings.useSelect((settings) => settings.environment);
    const health = useResourceEnabled(true, signozServicesR, { minutes });
    const dashboards = useResourceEnabled(true, signozDashboardsR);
    const [query, setQuery] = useState("");
    const [selected, setSelected] = useState(0);
    const mouseActive = useMouseActive();
    const listRef = useRef<HTMLDivElement>(null);

    const services = useMemo(() => mergeByService(health.data ?? [], environment).map((row) => row.service), [environment, health.data]);
    const items = useMemo(() => paletteItems(query, services, dashboards.data ?? []), [dashboards.data, query, services]);

    useEffect(() => {
        listRef.current?.querySelector<HTMLElement>(`.picker-item:nth-child(${selected + 1})`)?.scrollIntoView({ block: "nearest" });
    }, [selected]);

    const activate = (item: Item | undefined) => {
        if (!item || !paneId) return;
        item.run(paneId);
        closePalette();
    };

    const onKeyDown = (event: React.KeyboardEvent) => {
        if (event.key === "Escape") closePalette();
        else if (event.key === "ArrowDown" || (event.key === "Tab" && !event.shiftKey)) {
            event.preventDefault();
            setSelected((index) => (items.length ? (index + 1) % items.length : 0));
        } else if (event.key === "ArrowUp" || (event.key === "Tab" && event.shiftKey)) {
            event.preventDefault();
            setSelected((index) => (items.length ? (index - 1 + items.length) % items.length : 0));
        } else if (event.key === "Enter") {
            event.preventDefault();
            activate(items[selected]);
        }
    };

    return (
        <div className="picker-backdrop" onMouseDown={closePalette}>
            <div className="picker" onMouseDown={(event) => event.stopPropagation()}>
                <div className="picker-input-wrap">
                    <IconSearch size={15} className="picker-search-icon" />
                    <input
                        className="picker-input"
                        placeholder="Dashboards, services, actions, or paste a trace id…"
                        value={query}
                        onChange={(event) => {
                            setQuery(event.target.value);
                            setSelected(0);
                        }}
                        onKeyDown={onKeyDown}
                        autoFocus
                        spellCheck={false}
                    />
                </div>
                <div className="picker-list" ref={listRef}>
                    {items.length === 0 && <div className="picker-empty">no matches</div>}
                    {items.map((item, index) => (
                        <button
                            key={item.id}
                            type="button"
                            className={`picker-item${index === selected ? " sel" : ""}`}
                            onMouseEnter={() => {
                                if (mouseActive.current) setSelected(index);
                            }}
                            onClick={() => activate(item)}>
                            <span className="picker-icon command">
                                <SignozIcon size={14} />
                            </span>
                            <span className="picker-name">{item.label}</span>
                            {item.hint && (
                                <span className="picker-tags">
                                    <span className="picker-tag type">{item.hint}</span>
                                </span>
                            )}
                        </button>
                    ))}
                </div>
            </div>
        </div>
    );
}
