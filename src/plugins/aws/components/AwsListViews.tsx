import { useEffect, type ReactNode } from "react";
import { useResourceEnabled, type ResourceHandle } from "../../../plugin-api/resources";
import { IconChevron } from "../../../plugin-api/ui";
import type { Ec2Instance, LambdaFn, S3Bucket, SqsQueue } from "../api";
import { billingMonthsR, ec2InstancesR, lambdaFnsR, s3BucketsR, sqsQueuesR } from "../resources";
import { setBillingExpandedMonth, useAws } from "../state";
import { AwsRefresh } from "./AwsRefresh";

type Column<T> = {
    header: string;
    className?: string;
    cell: (row: T) => ReactNode;
};

function Frame<T>({
    handle,
    loading,
    emptyText,
    children,
}: {
    handle: ResourceHandle<T[]>;
    loading: string;
    emptyText: string;
    children: (data: T[]) => ReactNode;
}) {
    if (handle.data === undefined) {
        if (handle.status === "error" && handle.error) return <div className="aws-err">{handle.error}</div>;
        return <div className="aws-loading">{loading}</div>;
    }
    if (handle.data.length === 0) return <div className="aws-empty">{emptyText}</div>;
    return <>{children(handle.data)}</>;
}

function AwsTable<T>({ data, columns, rowKey }: { data: T[]; columns: Column<T>[]; rowKey: (row: T) => string }) {
    return (
        <table className="aws-table">
            <thead>
                <tr>
                    {columns.map((c) => (
                        <th key={c.header} className={c.className}>
                            {c.header}
                        </th>
                    ))}
                </tr>
            </thead>
            <tbody>
                {data.map((row) => (
                    <tr key={rowKey(row)} className="aws-row">
                        {columns.map((c) => (
                            <td key={c.header} className={c.className}>
                                {c.cell(row)}
                            </td>
                        ))}
                    </tr>
                ))}
            </tbody>
        </table>
    );
}

function TableView<T>({
    handle,
    loading,
    emptyText = "no results",
    columns,
    rowKey,
}: {
    handle: ResourceHandle<T[]>;
    loading: string;
    emptyText?: string;
    columns: Column<T>[];
    rowKey: (row: T) => string;
}) {
    return (
        <div className="aws-view">
            <AwsRefresh handle={handle} />
            <Frame handle={handle} loading={loading} emptyText={emptyText}>
                {(data) => <AwsTable data={data} columns={columns} rowKey={rowKey} />}
            </Frame>
        </div>
    );
}

const EC2_COLUMNS: Column<Ec2Instance>[] = [
    {
        header: "Instance",
        className: "aws-col-name",
        cell: (i) => (
            <>
                {i.name ?? i.instance_id}
                {i.name && <span className="aws-sub-id">{i.instance_id}</span>}
            </>
        ),
    },
    { header: "Type", cell: (i) => i.instance_type ?? "—" },
    { header: "State", cell: (i) => <span className={`aws-status aws-status-${i.state === "running" ? "ok" : "off"}`}>{i.state ?? "—"}</span> },
    { header: "Private IP", cell: (i) => i.private_ip ?? "—" },
    { header: "Public IP", cell: (i) => i.public_ip ?? "—" },
];

const LAMBDA_COLUMNS: Column<LambdaFn>[] = [
    { header: "Function", className: "aws-col-name", cell: (f) => f.name },
    { header: "Runtime", cell: (f) => f.runtime ?? "—" },
    { header: "Memory", cell: (f) => (f.memory_size ? `${f.memory_size} MB` : "—") },
    { header: "Timeout", cell: (f) => (f.timeout ? `${f.timeout}s` : "—") },
    { header: "Last Modified", cell: (f) => f.last_modified?.replace(/\..*$/, "") ?? "—" },
];

const SQS_COLUMNS: Column<SqsQueue>[] = [
    { header: "Queue", className: "aws-col-name", cell: (q) => q.name },
    { header: "URL", className: "aws-sub-id", cell: (q) => q.url },
];

const S3_COLUMNS: Column<S3Bucket>[] = [
    { header: "Bucket", className: "aws-col-name", cell: (b) => b.name },
    { header: "Created", cell: (b) => b.created_at?.replace(/\..*$/, "") ?? "—" },
];

export function AwsEc2View({ profile, active }: { profile: string; active: boolean }) {
    const handle = useResourceEnabled(active, ec2InstancesR, profile);
    return <TableView handle={handle} loading="loading instances…" columns={EC2_COLUMNS} rowKey={(i) => i.instance_id} />;
}

export function AwsLambdaView({ profile, active }: { profile: string; active: boolean }) {
    const handle = useResourceEnabled(active, lambdaFnsR, profile);
    return <TableView handle={handle} loading="loading functions…" columns={LAMBDA_COLUMNS} rowKey={(f) => f.name} />;
}

export function AwsSqsView({ profile, active }: { profile: string; active: boolean }) {
    const handle = useResourceEnabled(active, sqsQueuesR, profile);
    return <TableView handle={handle} loading="loading queues…" columns={SQS_COLUMNS} rowKey={(q) => q.url} />;
}

const MONTH_NAMES = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

function formatMonth(periodStart: string): string {
    const [y, m] = periodStart.split("-");
    const idx = parseInt(m, 10) - 1;
    if (idx < 0 || idx > 11) return periodStart;
    return `${MONTH_NAMES[idx]} ${y.slice(2)}`;
}

function formatAmount(amount: string | number, unit: string): string {
    const raw = typeof amount === "number" ? amount : Number(amount);
    if (!Number.isFinite(raw)) return `${unit} ${amount}`;
    const n = Math.abs(raw) < 0.005 ? 0 : raw;
    const symbol = unit === "USD" ? "$" : `${unit} `;
    const sign = n < 0 ? "-" : "";
    const body = Math.abs(n).toLocaleString("en-US", {
        minimumFractionDigits: 2,
        maximumFractionDigits: 2,
    });
    return `${sign}${symbol}${body}`;
}

function splitCharges(by_service: { amount: string }[]): {
    gross: number;
    credits: number;
    net: number;
} {
    let gross = 0;
    let credits = 0;
    for (const s of by_service) {
        const n = Number(s.amount);
        if (!Number.isFinite(n)) continue;
        if (n >= 0) gross += n;
        else credits += n; // negative
    }
    return { gross, credits, net: gross + credits };
}

export function AwsBillingView({ profile, active }: { profile: string; active: boolean }) {
    const handle = useResourceEnabled(active, billingMonthsR, profile, 5);
    const expanded = useAws((s) => s.expandedBillingMonth[profile] ?? null);
    const data = handle.data;

    useEffect(() => {
        if (!data || expanded !== null) return;
        const cur = data.find((m) => m.is_current);
        if (cur) setBillingExpandedMonth(profile, cur.period_start);
    }, [data, expanded, profile]);

    if (!data) {
        if (handle.status === "error" && handle.error)
            return (
                <div className="aws-view">
                    <AwsRefresh handle={handle} />
                    <div className="aws-err">{handle.error}</div>
                </div>
            );
        return (
            <div className="aws-view">
                <AwsRefresh handle={handle} />
                <div className="aws-loading">loading costs…</div>
            </div>
        );
    }
    if (data.length === 0)
        return (
            <div className="aws-view">
                <AwsRefresh handle={handle} />
                <div className="aws-empty">no billing data</div>
            </div>
        );

    const months = data;
    const splits = months.map((m) => splitCharges(m.by_service));
    const grosses = splits.map((s) => s.gross);
    const maxGross = Math.max(...grosses, 1);
    const currentIdx = months.findIndex((m) => m.is_current);
    const current = (currentIdx >= 0 ? months[currentIdx] : null) ?? months[months.length - 1];
    const currentSplit = currentIdx >= 0 ? splits[currentIdx] : splits[splits.length - 1];
    const avg = grosses.reduce((a, b) => a + b, 0) / months.length;

    return (
        <div className="aws-view aws-billing">
            <AwsRefresh handle={handle} />
            <div className="aws-billing-hero">
                <div className="aws-billing-hero-main">
                    <div className="aws-billing-period">{formatMonth(current.period_start)} · gross charges</div>
                    <div className="aws-billing-total">
                        <span className="aws-billing-amount">{formatAmount(currentSplit.gross, current.unit)}</span>
                    </div>
                </div>
                <div className="aws-billing-hero-stats">
                    {Math.abs(currentSplit.credits) > 0.005 && (
                        <>
                            <div className="aws-billing-stat credit">
                                <div className="aws-billing-stat-label">credits</div>
                                <div className="aws-billing-stat-value">{formatAmount(currentSplit.credits, current.unit)}</div>
                            </div>
                            <div className="aws-billing-stat">
                                <div className="aws-billing-stat-label">net</div>
                                <div className="aws-billing-stat-value">{formatAmount(currentSplit.net, current.unit)}</div>
                            </div>
                        </>
                    )}
                    <div className="aws-billing-stat">
                        <div className="aws-billing-stat-label">{months.length}mo avg</div>
                        <div className="aws-billing-stat-value">{formatAmount(avg, current.unit)}</div>
                    </div>
                </div>
            </div>

            <div className="aws-billing-timeline">
                {months.map((m, i) => {
                    const isOpen = expanded === m.period_start;
                    const split = splits[i];
                    const pct = (split.gross / maxGross) * 100;
                    const prev = i > 0 ? splits[i - 1].gross : null;
                    const delta = prev != null && prev > 0 ? ((split.gross - prev) / prev) * 100 : null;
                    const hasCredits = Math.abs(split.credits) > 0.005;
                    return (
                        <div key={m.period_start} className={`aws-bill-month${isOpen ? " open" : ""}${m.is_current ? " current" : ""}`}>
                            <button className="aws-bill-month-head" onClick={() => setBillingExpandedMonth(profile, isOpen ? null : m.period_start)}>
                                <span className="aws-bill-chev">
                                    <IconChevron size={11} />
                                </span>
                                <span className="aws-bill-month-name">
                                    {formatMonth(m.period_start)}
                                    {m.is_current && <span className="aws-bill-now">MTD</span>}
                                </span>
                                <span className="aws-bill-bar">
                                    <span className="aws-bill-bar-fill" style={{ width: `${pct}%` }} />
                                </span>
                                {delta !== null && Math.abs(delta) >= 1 && (
                                    <span className={`aws-bill-delta ${delta > 0 ? "up" : "down"}`}>
                                        {delta > 0 ? "▲" : "▼"}
                                        {Math.abs(delta).toFixed(0)}%
                                    </span>
                                )}
                                <span className="aws-bill-amount">{formatAmount(split.gross, m.unit)}</span>
                            </button>
                            {isOpen && (
                                <div className={`aws-bill-services${hasCredits ? " split" : ""}`}>
                                    <div className="aws-bill-col">
                                        <div className="aws-bill-col-head">Charges</div>
                                        {m.by_service
                                            .filter((s) => Number(s.amount) > 0.005)
                                            .map((s) => (
                                                <div className="aws-bill-svc" key={`c-${s.service}`}>
                                                    <span className="aws-bill-svc-name">{s.service}</span>
                                                    <span className="aws-bill-svc-bar">
                                                        <span
                                                            className="aws-bill-svc-bar-fill"
                                                            style={{
                                                                width: `${(Number(s.amount) / Math.max(split.gross, 1)) * 100}%`,
                                                            }}
                                                        />
                                                    </span>
                                                    <span className="aws-bill-svc-amount">{formatAmount(s.amount, s.unit)}</span>
                                                </div>
                                            ))}
                                        {m.by_service.length === 0 && <div className="aws-bill-empty">no charges this month</div>}
                                    </div>

                                    {hasCredits && (
                                        <div className="aws-bill-col">
                                            <div className="aws-bill-col-head">Credits &amp; refunds</div>
                                            {m.by_service
                                                .filter((s) => Number(s.amount) < -0.005)
                                                .sort((a, b) => Number(a.amount) - Number(b.amount))
                                                .map((s) => (
                                                    <div className="aws-bill-svc credit" key={`r-${s.service}`}>
                                                        <span className="aws-bill-svc-name">{s.service}</span>
                                                        <span className="aws-bill-svc-bar">
                                                            <span
                                                                className="aws-bill-svc-bar-fill credit"
                                                                style={{
                                                                    width: `${Math.min(
                                                                        (Math.abs(Number(s.amount)) / Math.max(Math.abs(split.credits), 1)) * 100,
                                                                        100,
                                                                    )}%`,
                                                                }}
                                                            />
                                                        </span>
                                                        <span className="aws-bill-svc-amount credit">{formatAmount(s.amount, s.unit)}</span>
                                                    </div>
                                                ))}
                                            <div className="aws-bill-svc total">
                                                <span className="aws-bill-svc-name">Net for month</span>
                                                <span className="aws-bill-svc-bar" />
                                                <span className="aws-bill-svc-amount">{formatAmount(split.net, m.unit)}</span>
                                            </div>
                                        </div>
                                    )}
                                </div>
                            )}
                        </div>
                    );
                })}
            </div>
        </div>
    );
}

export function AwsS3View({ profile, active }: { profile: string; active: boolean }) {
    const handle = useResourceEnabled(active, s3BucketsR, profile);
    return <TableView handle={handle} loading="loading buckets…" columns={S3_COLUMNS} rowKey={(b) => b.name} />;
}
