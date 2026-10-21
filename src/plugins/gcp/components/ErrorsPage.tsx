import { errorText } from "../../../plugin-api/format";
import { useState } from "react";
import { notify } from "../../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../../plugin-api/resources";
import { Dropdown, SendToAgentMenu } from "../../../plugin-api/ui";
import { gcpApi, type ErrorGroup, type ErrorSample, type ErrorStatus, type Target } from "../api";
import { errorEventsR, errorGroupsR } from "../resources";
import { updateGcpView, useGcpView } from "../state";
import type { ProductProps } from "./GcpPane";
import { Filter } from "./RunPage";
import { ago, Button, consoleUrl, Facts, keyOf, Loaded, PageHeader, Table, Tabs, visit } from "./parts";

const PERIODS = [
    { value: "1h", label: "1 hour" },
    { value: "6h", label: "6 hours" },
    { value: "1d", label: "1 day" },
    { value: "7d", label: "7 days" },
    { value: "30d", label: "30 days" },
] as const;

const STATUSES: { value: ErrorStatus; label: string }[] = [
    { value: "OPEN", label: "Open" },
    { value: "ACKNOWLEDGED", label: "Acknowledged" },
    { value: "RESOLVED", label: "Resolved" },
    { value: "MUTED", label: "Muted" },
];

/** An error group as an agent reads it: what it is, how often, where it was thrown and its stack. */
export function errorForAgent(group: ErrorGroup): string {
    const sample = group.sample;
    return [
        `Error Reporting group ${group.id}: ${group.title}`,
        [
            `${group.count ?? "?"} occurrences, ${group.users ?? "?"} users, status ${group.status.toLowerCase()}`,
            group.services.length > 0 && `Services: ${group.services.join(", ")}`,
            sample.file && `Thrown at ${sample.file}:${sample.line ?? "?"}${sample.function ? ` in ${sample.function}` : ""}`,
            sample.request && `Request: ${sample.request.method ?? ""} ${sample.request.url ?? ""} → ${sample.request.status ?? ""}`,
            `First seen ${group.firstSeen ?? "?"}, last seen ${group.lastSeen ?? "?"}`,
        ]
            .filter(Boolean)
            .join("\n"),
        `Stack:\n\n\`\`\`\n${sample.message}\n\`\`\``,
        `To mark it resolved once fixed, use gcp_act with action error_resolve and name ${group.id}.`,
    ].join("\n\n");
}

function Timeline({ counts }: { counts: number[] }) {
    const max = Math.max(1, ...counts);
    return (
        <span className="gcp-timeline" aria-hidden="true">
            {counts.slice(-24).map((count, index) => (
                <i key={index} style={{ height: `${Math.max(8, (count / max) * 100)}%` }} />
            ))}
        </span>
    );
}

function useStatusChange(target: Target) {
    return (group: ErrorGroup, status: ErrorStatus) =>
        void gcpApi
            .errorStatus(target, group.id, status)
            .then(() => {
                notify("success", `Marked ${status.toLowerCase()}`);
                invalidate((kind) => kind === "gcp.errors");
            })
            .catch((error: unknown) => notify("error", `Change status: ${errorText(error)}`));
}

export function ErrorsPage({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    const since = PERIODS.find((period) => period.value === view.list)?.value ?? "1d";
    const groups = useResourceEnabled(active, errorGroupsR, ...keyOf(target), since);
    const setStatus = useStatusChange(target);
    const open = groups.data?.find((group) => group.id === view.open);
    if (open)
        return (
            <GroupPage
                active={active}
                target={target}
                group={open}
                onBack={() => updateGcpView(paneId, { open: null })}
                onStatus={(status) => setStatus(open, status)}
            />
        );
    const words = view.filter.trim().toLowerCase();
    return (
        <div className="gcp-page">
            <PageHeader title="Error Reporting">
                <Button onClick={() => visit(consoleUrl("errors", target.project))}>Open in Cloud console</Button>
            </PageHeader>
            <Tabs label="Time range" tabs={PERIODS} value={since} onChange={(value) => updateGcpView(paneId, { list: value })} />
            <Filter paneId={paneId} placeholder="Filter errors" />
            <Loaded handle={groups} label="errors" empty={(rows) => (rows.length === 0 ? "No errors were reported in this time range." : null)}>
                {(rows) => (
                    <Table
                        label="Error groups"
                        rows={rows.filter(
                            (group) =>
                                !words || group.title.toLowerCase().includes(words) || group.services.some((service) => service.includes(words)),
                        )}
                        rowKey={(group) => group.id}
                        onOpen={(group: ErrorGroup) => updateGcpView(paneId, { open: group.id })}
                        columns={[
                            {
                                label: "Occurrences",
                                width: "minmax(130px, auto)",
                                cell: (group) => (
                                    <span className="gcp-occurrences">
                                        <b>{group.count ?? "—"}</b>
                                        <Timeline counts={group.timeline} />
                                    </span>
                                ),
                            },
                            { label: "Users", width: "60px", cell: (group) => group.users ?? "—" },
                            { label: "Error", width: "minmax(220px, 2.4fr)", cell: (group) => <span className="gcp-name">{group.title}</span> },
                            { label: "Seen in", width: "minmax(100px, 1fr)", cell: (group) => group.services.join(", ") || "—" },
                            { label: "Last seen", width: "90px", cell: (group) => ago(group.lastSeen) },
                            {
                                label: "Status",
                                width: "110px",
                                cell: (group) => (
                                    <span className={`gcp-error-status ${group.status}`}>
                                        {STATUSES.find((s) => s.value === group.status)?.label}
                                    </span>
                                ),
                            },
                        ]}
                    />
                )}
            </Loaded>
        </div>
    );
}

function GroupPage({
    active,
    target,
    group,
    onBack,
    onStatus,
}: {
    active: boolean;
    target: Target;
    group: ErrorGroup;
    onBack: () => void;
    onStatus: (status: ErrorStatus) => void;
}) {
    const events = useResourceEnabled(active, errorEventsR, ...keyOf(target), group.id);
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
    const [sample, setSample] = useState<ErrorSample>(group.sample);
    return (
        <div className="gcp-page">
            <PageHeader title={group.title} back={{ label: "errors", go: onBack }}>
                <Dropdown label="Resolution status" value={group.status} options={STATUSES} onChange={(value) => onStatus(value as ErrorStatus)} />
                <button type="button" className="gcp-button" onClick={(event) => setMenu({ x: event.clientX, y: event.clientY })}>
                    Send to agent
                </button>
                {group.issue && <Button onClick={() => visit(group.issue!)}>Tracking issue</Button>}
            </PageHeader>
            <Facts
                items={[
                    ["Occurrences", group.count],
                    ["Users affected", group.users],
                    ["Seen in", group.services.join(", ")],
                    ["First seen", ago(group.firstSeen)],
                    ["Last seen", ago(group.lastSeen)],
                    ["Thrown at", sample.file && `${sample.file}:${sample.line ?? "?"}${sample.function ? ` (${sample.function})` : ""}`],
                    ["Request", sample.request && `${sample.request.method ?? ""} ${sample.request.url ?? ""} → ${sample.request.status ?? ""}`],
                    ["Version", sample.version],
                ]}
            />
            <h2 className="gcp-section-title">Stack trace</h2>
            <pre className="gcp-terminal">{sample.message}</pre>
            <h2 className="gcp-section-title">Recent samples</h2>
            <Loaded handle={events} label="samples" empty={(rows) => (rows.length === 0 ? "No samples in the last 30 days." : null)}>
                {(rows) => (
                    <Table
                        label="Samples"
                        rows={rows}
                        rowKey={(row) => `${row.at ?? ""}-${row.service ?? ""}`}
                        onOpen={setSample}
                        columns={[
                            { label: "When", width: "100px", cell: (row) => ago(row.at) },
                            { label: "Service", cell: (row) => [row.service, row.version].filter(Boolean).join(" ") || "—" },
                            {
                                label: "Request",
                                width: "minmax(160px, 1.6fr)",
                                cell: (row) => (row.request ? `${row.request.method ?? ""} ${row.request.url ?? ""}` : "—"),
                            },
                            { label: "Status", width: "70px", cell: (row) => row.request?.status ?? "—" },
                        ]}
                    />
                )}
            </Loaded>
            {menu && (
                <SendToAgentMenu
                    x={menu.x}
                    y={menu.y}
                    alignRight
                    delivery={() => ({ text: errorForAgent({ ...group, sample }) })}
                    onClose={() => setMenu(null)}
                />
            )}
        </div>
    );
}
