import { Fragment, useEffect, useRef, type KeyboardEvent, type ReactNode } from "react";
import { copyText, notify, openSignInUrl, reportError } from "../../../plugin-api/host";
import { useResource, type ResourceHandle } from "../../../plugin-api/resources";
import { IconChevron, IconCopy, IconRefresh, IconSearch } from "../../../plugin-api/ui";
import { awsProfilesR } from "../resources";
import { setAwsCount, type AwsService } from "../state";
import { IconExternal } from "./icons";

export type Health = "ok" | "warn" | "fail" | "off";

export function relative(iso: string | null | undefined): string {
    if (!iso) return "—";
    const t = Date.parse(iso);
    if (Number.isNaN(t)) return "—";
    const d = Math.max(0, (Date.now() - t) / 1000);
    if (d < 60) return "just now";
    if (d < 3600) return `${Math.floor(d / 60)}m ago`;
    if (d < 86400) return `${Math.floor(d / 3600)}h ago`;
    if (d < 86400 * 30) return `${Math.floor(d / 86400)}d ago`;
    if (d < 86400 * 365) return `${Math.floor(d / (86400 * 30))}mo ago`;
    return `${Math.floor(d / (86400 * 365))}y ago`;
}

export function useRegion(profile: string): string {
    const profiles = useResource(awsProfilesR).data;
    return profiles?.find((p) => p.name === profile)?.region ?? "us-east-1";
}

export function openConsole(url: string): void {
    void openSignInUrl(url).catch(reportError("open the AWS console"));
}

export function copy(value: string, label: string): void {
    void copyText(value).then(() => notify("success", `copied ${label}`), reportError("copy"));
}

/** Tells the sidebar how many things this service listed. */
export function useReportCount(profile: string, service: AwsService, count: string | number | undefined): void {
    useEffect(() => {
        if (count !== undefined) setAwsCount(profile, service, String(count));
    }, [profile, service, count]);
}

export function Crumbs({ profile, region, trail }: { profile: string; region: string; trail: { label: string; onClick?: () => void }[] }) {
    return (
        <nav className="aws-crumbs" aria-label="Location">
            <span className="aws-crumbs-profile">{profile}</span>
            <IconChevron size={10} />
            <span>{region}</span>
            {trail.map((part, i) => (
                <span key={i} className="aws-crumbs-part">
                    <IconChevron size={10} />
                    {part.onClick ? (
                        <button className="aws-crumbs-link" onClick={part.onClick}>
                            {part.label}
                        </button>
                    ) : (
                        <span>{part.label}</span>
                    )}
                </span>
            ))}
        </nav>
    );
}

export function Header({
    crumbs,
    title,
    count,
    aside,
    tools,
    handle,
}: {
    crumbs: ReactNode;
    title: ReactNode;
    count?: number;
    aside?: ReactNode;
    tools?: ReactNode;
    handle?: ResourceHandle<unknown>;
}) {
    return (
        <header className="aws-head">
            {crumbs}
            <div className="aws-head-row">
                <h2 className="aws-title">
                    {title}
                    {count !== undefined && <span className="aws-count">{count}</span>}
                </h2>
                {aside}
                <div className="aws-head-tools">
                    {tools}
                    {handle && <Refresh handle={handle} />}
                </div>
            </div>
        </header>
    );
}

export function Refresh({ handle }: { handle: ResourceHandle<unknown> }) {
    const busy = handle.status === "loading";
    return (
        <button
            className={`aws-btn aws-icon-btn aws-refresh${busy ? " busy" : ""}`}
            onClick={() => void handle.refresh()}
            disabled={busy}
            title={busy ? "Refreshing…" : "Refresh from AWS"}
            aria-label="Refresh">
            <IconRefresh size={14} />
        </button>
    );
}

export interface SegOption<V extends string> {
    value: V;
    label: string;
    count?: number;
}

export function Seg<V extends string>({ options, value, onChange }: { options: SegOption<V>[]; value: V; onChange: (value: V) => void }) {
    return (
        <div className="aws-seg" role="group">
            {options.map((o) => (
                <button key={o.value} className={o.value === value ? "on" : ""} aria-pressed={o.value === value} onClick={() => onChange(o.value)}>
                    {o.label}
                    {o.count !== undefined && <span className="aws-seg-count">{o.count}</span>}
                </button>
            ))}
        </div>
    );
}

export function Filter({ value, onChange, placeholder }: { value: string; onChange: (value: string) => void; placeholder: string }) {
    return (
        <label className="aws-filter">
            <IconSearch size={14} />
            <input
                value={value}
                placeholder={placeholder}
                spellCheck={false}
                onChange={(e) => onChange(e.target.value)}
                onKeyDown={(e) => {
                    if (e.key === "Escape" && value) {
                        e.stopPropagation();
                        onChange("");
                    }
                }}
            />
            <kbd>/</kbd>
        </label>
    );
}

export function matches(query: string, ...fields: (string | null | undefined)[]): boolean {
    const q = query.trim().toLowerCase();
    if (!q) return true;
    return fields.some((f) => f?.toLowerCase().includes(q));
}

const HEALTH_LABEL: Record<Health, string> = { ok: "Active", warn: "Pending", fail: "Failed", off: "Inactive" };

export function State({ health, label }: { health: Health; label?: string }) {
    return (
        <span className={`aws-state ${health}`}>
            <span className="aws-dot" />
            {label ?? HEALTH_LABEL[health]}
        </span>
    );
}

/** One square per task: running, then pending, then the empty slots it is still short of. */
export function TaskMeter({ running, pending, target, wrap }: { running: number; pending: number; target: number; wrap?: boolean }) {
    const cells: string[] = [];
    for (let i = 0; i < running; i++) cells.push("run");
    for (let i = 0; i < pending; i++) cells.push("pend");
    for (let i = running + pending; i < target; i++) cells.push("gap");
    const shown = cells.slice(0, 64);
    return (
        <span className={`aws-meter${wrap ? " wrap" : ""}`} title={`${running} running · ${pending} pending${target ? ` · ${target} wanted` : ""}`}>
            {shown.map((kind, i) => (
                <i key={i} className={kind} />
            ))}
        </span>
    );
}

export function Tag({ children, tone }: { children: ReactNode; tone?: "warn" }) {
    return <span className={`aws-tag${tone ? ` ${tone}` : ""}`}>{children}</span>;
}

export function Cell({ children, dim }: { children: ReactNode; dim?: boolean }) {
    return <span className={`aws-cell${dim ? " dim" : ""}`}>{children}</span>;
}

export interface Column<T> {
    header: string;
    width: string;
    align?: "right";
    cell: (row: T) => ReactNode;
}

export function Table<T>({
    columns,
    rows,
    rowKey,
    selected,
    onSelect,
    onOpen,
    rowClass,
    label,
}: {
    columns: Column<T>[];
    rows: T[];
    rowKey: (row: T) => string;
    selected?: string | null;
    onSelect?: (row: T) => void;
    onOpen?: (row: T) => void;
    rowClass?: (row: T) => string | undefined;
    label: string;
}) {
    const body = useRef<HTMLTableSectionElement>(null);
    const focusRow = (index: number) => {
        const row = body.current?.children[index] as HTMLElement | undefined;
        row?.focus();
    };
    const onKey = (event: KeyboardEvent, row: T, index: number) => {
        if (event.key === "Enter") {
            event.preventDefault();
            (onOpen ?? onSelect)?.(row);
        } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            const next = Math.max(0, Math.min(rows.length - 1, index + (event.key === "ArrowDown" ? 1 : -1)));
            onSelect?.(rows[next]);
            focusRow(next);
        }
    };
    const cols = (
        <colgroup>
            {columns.map((c, i) => (
                <col key={i} style={{ width: c.width }} />
            ))}
        </colgroup>
    );
    return (
        <div className="aws-table-wrap">
            <div className="aws-table-head" aria-hidden="true">
                <table className="aws-table">
                    {cols}
                    <thead>
                        <tr>
                            {columns.map((c, i) => (
                                <th key={i} className={c.align === "right" ? "r" : undefined}>
                                    {c.header}
                                </th>
                            ))}
                        </tr>
                    </thead>
                </table>
            </div>
            <div className="aws-table-body">
                <table className="aws-table" aria-label={label}>
                    {cols}
                    <tbody ref={body}>
                        {rows.map((row, index) => {
                            const key = rowKey(row);
                            const isSelected = key === selected;
                            return (
                                <tr
                                    key={key}
                                    tabIndex={0}
                                    aria-selected={isSelected}
                                    className={[isSelected ? "sel" : "", rowClass?.(row) ?? ""].join(" ").trim() || undefined}
                                    onClick={() => (isSelected && onOpen ? onOpen(row) : onSelect ? onSelect(row) : onOpen?.(row))}
                                    onKeyDown={(event) => onKey(event, row, index)}>
                                    {columns.map((c, i) => (
                                        <td key={i} className={c.align === "right" ? "r" : undefined}>
                                            {c.cell(row)}
                                        </td>
                                    ))}
                                </tr>
                            );
                        })}
                    </tbody>
                </table>
            </div>
        </div>
    );
}

/** The loading, failed and empty states a list shows in place of its rows. */
export function ListState({
    handle,
    loading,
    empty,
    count,
}: {
    handle: ResourceHandle<unknown>;
    loading: string;
    empty: string;
    count: number | undefined;
}) {
    if (count === undefined) {
        if (handle.status === "error" && handle.error) return <div className="aws-note err">{handle.error}</div>;
        return <div className="aws-note">{loading}</div>;
    }
    if (count === 0) return <div className="aws-note">{empty}</div>;
    return null;
}

export function Inspector({ children }: { children: ReactNode }) {
    return <aside className="aws-insp">{children}</aside>;
}

export function InspectorHead({ title, badge, sub, actions }: { title: ReactNode; badge?: ReactNode; sub?: string; actions?: ReactNode }) {
    return (
        <div className="aws-insp-head">
            <div className="aws-insp-title">
                {title}
                {badge}
            </div>
            {sub && (
                <div className="aws-insp-sub">
                    {sub.split(/(?<=[:/])/).map((part, i) => (
                        <Fragment key={i}>
                            {part}
                            <wbr />
                        </Fragment>
                    ))}
                </div>
            )}
            {actions && <div className="aws-insp-actions">{actions}</div>}
        </div>
    );
}

export function ConsoleButton({ url }: { url: string }) {
    return (
        <button className="aws-btn" onClick={() => openConsole(url)} title="Open in the AWS console">
            <IconExternal />
            Console
        </button>
    );
}

export function Stats({ items }: { items: { label: string; value: ReactNode; tone?: "warn" | "zero" }[] }) {
    return (
        <div className="aws-stats" style={{ gridTemplateColumns: `repeat(${items.length}, minmax(0, 1fr))` }}>
            {items.map((item) => (
                <div key={item.label} className="aws-stat">
                    <span className="aws-stat-label">{item.label}</span>
                    <span className={`aws-stat-value${item.tone ? ` ${item.tone}` : ""}`}>{item.value}</span>
                </div>
            ))}
        </div>
    );
}

export function Facts({ items }: { items: { label: string; value: string | null | undefined; copy?: boolean }[] }) {
    return (
        <dl className="aws-facts">
            {items.map((item) => (
                <div key={item.label} className="aws-fact">
                    <dt>{item.label}</dt>
                    <dd title={item.value ?? undefined}>{item.value || "—"}</dd>
                    {item.copy && item.value ? (
                        <button
                            className="aws-fact-copy"
                            onClick={() => copy(item.value!, item.label.toLowerCase())}
                            title={`Copy ${item.label.toLowerCase()}`}
                            aria-label={`Copy ${item.label.toLowerCase()}`}>
                            <IconCopy size={13} />
                        </button>
                    ) : (
                        <span />
                    )}
                </div>
            ))}
        </dl>
    );
}

export function Section({ title, meta, children }: { title: string; meta?: ReactNode; children: ReactNode }) {
    return (
        <section className="aws-insp-sec">
            <h3>
                {title}
                {meta !== undefined && <span>{meta}</span>}
            </h3>
            {children}
        </section>
    );
}

export function InspectorEmpty({ text }: { text: string }) {
    return (
        <Inspector>
            <div className="aws-insp-empty">{text}</div>
        </Inspector>
    );
}
