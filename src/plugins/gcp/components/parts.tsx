import { useEffect, useMemo, useState, type ReactNode } from "react";
import { confirmDialog, copyWithNotice, notify, openUrl, swallow } from "../../../plugin-api/host";
import { errorText, formatAgo } from "../../../plugin-api/format";
import { type ResourceHandle } from "../../../plugin-api/resources";
import { EmptyState, SkeletonRows, StaleNotice } from "../../../plugin-api/ui";
import { refreshGcp } from "../resources";
import { type Series, type Target } from "../api";
import { gcpSettings } from "../state";
import { Problem } from "./Problem";

/** The account and project the console acts on, or null before a project is picked. */
export function useTarget(): Target | null {
    const account = gcpSettings.useSelect((s) => s.account);
    const project = gcpSettings.useSelect((s) => s.project);
    return useMemo(() => (project ? { account, project } : null), [account, project]);
}

/** The plain-string key resources are cached under. */
export const keyOf = (target: Target) => [target.account ?? "", target.project] as const;

export const ago = (at: string | null | undefined) => (at ? formatAgo(Date.parse(at)) : "—");

export const visit = (url: string) => void openUrl(url).catch(swallow("open the page"));

export const consoleUrl = (path: string, project: string) => `https://console.cloud.google.com/${path}?project=${encodeURIComponent(project)}`;

type Tone = "ok" | "error" | "busy" | "off" | "warn";

const TONES: Record<string, Tone> = {
    READY: "ok",
    RUNNING: "ok",
    RUNNABLE: "ok",
    ACTIVE: "ok",
    SUCCESS: "ok",
    SUCCEEDED: "ok",
    ENABLED: "ok",
    FAILED: "error",
    FAILURE: "error",
    ERROR: "error",
    INTERNAL_ERROR: "error",
    TIMEOUT: "error",
    DEGRADED: "warn",
    RECONCILING: "busy",
    DEPLOYING: "busy",
    PROVISIONING: "busy",
    STAGING: "busy",
    STOPPING: "busy",
    SUSPENDING: "busy",
    WORKING: "busy",
    QUEUED: "busy",
    PENDING: "busy",
    CREATING: "busy",
    MAINTENANCE: "busy",
    REPAIRING: "busy",
    TERMINATED: "off",
    STOPPED: "off",
    SUSPENDED: "off",
    CANCELLED: "off",
    DISABLED: "off",
    DESTROYED: "off",
    EXPIRED: "off",
};

export const toneOf = (state: string | null | undefined): Tone => TONES[(state ?? "").toUpperCase()] ?? "off";

const label = (state: string) => state.charAt(0) + state.slice(1).toLowerCase().replace(/_/g, " ");

/** A state as the Cloud console shows it: a green tick, a red mark, a turning wheel or a grey stop. */
export function Status({ state, text }: { state: string | null | undefined; text?: boolean }) {
    const tone = toneOf(state);
    return (
        <span className={`gcp-status ${tone}`} title={state ? label(state) : undefined}>
            <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
                {tone === "ok" && (
                    <>
                        <circle cx="8" cy="8" r="7" fill="currentColor" />
                        <path
                            d="m4.8 8.2 2.1 2.1 4.3-4.4"
                            fill="none"
                            stroke="var(--gcp-on-status)"
                            strokeWidth="1.6"
                            strokeLinecap="round"
                            strokeLinejoin="round"
                        />
                    </>
                )}
                {tone === "error" && (
                    <>
                        <circle cx="8" cy="8" r="7" fill="currentColor" />
                        <path d="M8 4.4v4.4M8 11.2v.1" stroke="var(--gcp-on-status)" strokeWidth="1.8" strokeLinecap="round" />
                    </>
                )}
                {tone === "warn" && (
                    <>
                        <path d="M8 1.5 15 14H1z" fill="currentColor" />
                        <path d="M8 6v3.6M8 11.6v.1" stroke="var(--gcp-on-status)" strokeWidth="1.6" strokeLinecap="round" />
                    </>
                )}
                {tone === "busy" && (
                    <circle className="gcp-spin" cx="8" cy="8" r="6" fill="none" stroke="currentColor" strokeWidth="2" strokeDasharray="26 12" />
                )}
                {tone === "off" && (
                    <>
                        <circle cx="8" cy="8" r="7" fill="none" stroke="currentColor" strokeWidth="1.6" />
                        <rect x="5.5" y="5.5" width="5" height="5" rx="0.6" fill="currentColor" />
                    </>
                )}
            </svg>
            {text && state && <span>{label(state)}</span>}
        </span>
    );
}

export interface Column<Row> {
    label: string;
    cell: (row: Row) => ReactNode;
    /** A CSS grid track, such as `minmax(140px, 1.4fr)`. */
    width?: string;
    align?: "end";
}

/** The Cloud console's table: a header row, then a row per item, each opening it when there is somewhere to go. */
export function Table<Row>({
    rows,
    columns,
    rowKey,
    onOpen,
    label: name,
    selected,
}: {
    rows: Row[];
    columns: Column<Row>[];
    rowKey: (row: Row) => string;
    onOpen?: (row: Row) => void;
    label: string;
    selected?: string | null;
}) {
    const template = columns.map((column) => column.width ?? "minmax(100px, 1fr)").join(" ");
    return (
        <div className="gcp-table" role="table" aria-label={name}>
            <div className="gcp-table-head" role="row" style={{ gridTemplateColumns: template }}>
                {columns.map((column) => (
                    <span key={column.label} role="columnheader" className={column.align === "end" ? "end" : undefined}>
                        {column.label}
                    </span>
                ))}
            </div>
            {rows.map((row) => {
                const key = rowKey(row);
                const cells = columns.map((column) => (
                    <span key={column.label} role="cell" className={`gcp-cell${column.align === "end" ? " end" : ""}`}>
                        {column.cell(row)}
                    </span>
                ));
                return onOpen ? (
                    <button
                        key={key}
                        type="button"
                        role="row"
                        className={`gcp-table-row${selected === key ? " selected" : ""}`}
                        style={{ gridTemplateColumns: template }}
                        onClick={() => onOpen(row)}>
                        {cells}
                    </button>
                ) : (
                    <div key={key} role="row" className="gcp-table-row" style={{ gridTemplateColumns: template }}>
                        {cells}
                    </div>
                );
            })}
        </div>
    );
}

/** What a resource shows while it loads, when it fails, when it is empty, and once it has rows. */
export function Loaded<T>({
    handle,
    empty,
    label: name,
    children,
}: {
    handle: ResourceHandle<T>;
    empty: (data: T) => string | null;
    label: string;
    children: (data: T) => ReactNode;
}) {
    if (handle.status === "error" && handle.data === undefined)
        return <Problem error={handle.error} what={name} onRetry={() => void handle.refresh()} />;
    if (handle.data === undefined) return <SkeletonRows rows={6} label={`Loading ${name}`} />;
    const nothing = empty(handle.data);
    return (
        <>
            <StaleNotice handle={handle} />
            {nothing ? <EmptyState message={nothing} /> : children(handle.data)}
        </>
    );
}

export function Tabs<T extends string>({
    tabs,
    value,
    onChange,
    label: name,
}: {
    tabs: readonly { value: T; label: string }[];
    value: T;
    onChange: (value: T) => void;
    label: string;
}) {
    return (
        <div className="gcp-tabs" role="tablist" aria-label={name}>
            {tabs.map((tab) => (
                <button
                    key={tab.value}
                    type="button"
                    role="tab"
                    aria-selected={tab.value === value}
                    className={tab.value === value ? "on" : undefined}
                    onClick={() => onChange(tab.value)}>
                    {tab.label}
                </button>
            ))}
        </div>
    );
}

/** A page's title row, as the Cloud console draws it: a way back, the title, and the page's actions. */
export function PageHeader({ title, back, children }: { title: ReactNode; back?: { label: string; go: () => void }; children?: ReactNode }) {
    return (
        <header className="gcp-page-head">
            {back && (
                <button
                    type="button"
                    className="gcp-icon-button"
                    aria-label={`Back to ${back.label}`}
                    title={`Back to ${back.label}`}
                    onClick={back.go}>
                    <svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true">
                        <path d="M20 11H7.8l5.6-5.6L12 4l-8 8 8 8 1.4-1.4L7.8 13H20z" fill="currentColor" />
                    </svg>
                </button>
            )}
            <h1 className="gcp-page-title">{title}</h1>
            <span className="gcp-page-actions">{children}</span>
        </header>
    );
}

/** Label and value pairs, as a details card lists them. */
export function Facts({ items }: { items: [string, ReactNode][] }) {
    return (
        <dl className="gcp-facts">
            {items
                .filter(([, value]) => value !== null && value !== undefined && value !== "")
                .map(([name, value]) => (
                    <div key={name}>
                        <dt>{name}</dt>
                        <dd>{value}</dd>
                    </div>
                ))}
        </dl>
    );
}

export function Copyable({ text, what }: { text: string; what: string }) {
    return (
        <button type="button" className="gcp-copyable" title={`Copy ${what}`} onClick={() => copyWithNotice(text, what)}>
            <span>{text}</span>
            <svg width="14" height="14" viewBox="0 0 24 24" aria-hidden="true">
                <path
                    d="M16 1H4a2 2 0 0 0-2 2v14h2V3h12zm3 4H8a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2m0 16H8V7h11z"
                    fill="currentColor"
                />
            </svg>
        </button>
    );
}

export function Button({
    children,
    onClick,
    disabled,
    primary,
    danger,
    title,
}: {
    children: ReactNode;
    onClick: () => void;
    disabled?: boolean;
    primary?: boolean;
    danger?: boolean;
    title?: string;
}) {
    return (
        <button
            type="button"
            className={`gcp-button${primary ? " primary" : ""}${danger ? " danger" : ""}`}
            disabled={disabled}
            title={title}
            onClick={onClick}>
            {children}
        </button>
    );
}

/** A small line chart of a series, with its latest value. */
export function Sparkline({ series, title }: { series: Series | undefined; title: string }) {
    const points = series?.points ?? [];
    const width = 220;
    const height = 48;
    const values = points.map((point) => point.value);
    const max = Math.max(1e-9, ...values);
    const path = points
        .map(
            (point, index) =>
                `${index === 0 ? "M" : "L"}${((index / Math.max(1, points.length - 1)) * width).toFixed(1)},${(height - (point.value / max) * (height - 4) - 2).toFixed(1)}`,
        )
        .join(" ");
    const latest = values.length > 0 ? values[values.length - 1] : null;
    return (
        <figure className="gcp-chart">
            <figcaption>
                <span>{title}</span>
                <b>{latest === null ? "—" : `${Number(latest.toFixed(latest < 10 ? 2 : 0))} ${series?.unit ?? ""}`}</b>
            </figcaption>
            <svg viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none" role="img" aria-label={`${title} over time`}>
                {points.length > 1 ? (
                    <path d={path} />
                ) : (
                    <text x="50%" y="55%" textAnchor="middle">
                        No data
                    </text>
                )}
            </svg>
        </figure>
    );
}

/** Runs a change only once the person confirms it, saying how it went and reading fresh answers after. */
export function useAction() {
    const [busy, setBusy] = useState(false);
    const run = async (
        confirm: { title: string; body: string; confirmLabel: string; destructive?: boolean },
        done: string,
        change: () => Promise<unknown>,
    ) => {
        if (busy || !(await confirmDialog(confirm))) return false;
        setBusy(true);
        try {
            await change();
            notify("success", done);
            refreshGcp();
            return true;
        } catch (error) {
            notify("error", `${confirm.confirmLabel}: ${errorText(error)}`);
            return false;
        } finally {
            setBusy(false);
        }
    };
    return { busy, run };
}

/** Bytes as KiB, MiB or GiB. */
export function size(bytes: number | null | undefined): string {
    if (bytes === null || bytes === undefined) return "—";
    const units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let value = bytes;
    let unit = 0;
    while (value >= 1024 && unit < units.length - 1) {
        value /= 1024;
        unit += 1;
    }
    return `${unit === 0 ? value : value.toFixed(1)} ${units[unit]}`;
}

/** How long something took, from two times. */
export function took(start: string | null | undefined, end: string | null | undefined): string {
    if (!start) return "—";
    const ms = (end ? Date.parse(end) : Date.now()) - Date.parse(start);
    if (!Number.isFinite(ms) || ms < 0) return "—";
    const seconds = Math.round(ms / 1000);
    if (seconds < 60) return `${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    return minutes < 60 ? `${minutes}m ${seconds % 60}s` : `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

/** Reads a resource again every few seconds while something in it is still on its way. */
export function usePollWhile(active: boolean, underway: boolean, refresh: () => unknown, everyMs = 4000) {
    useEffect(() => {
        if (!active || !underway) return;
        const timer = window.setInterval(() => void refresh(), everyMs);
        return () => window.clearInterval(timer);
    }, [active, underway, refresh, everyMs]);
}
