import { useState } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { Filter, IconCopy, Seg } from "../../../plugin-api/ui";
import { ec2V, lambdaV, s3V, sqsV } from "../resources";
import { selectAws, setLambdaLogs, useAws } from "../state";
import type { AwsLambdaFunction, AwsS3Bucket } from "../types.generated";
import { AwsLogTailView } from "./AwsLogTailView";
import { IconLogs } from "./icons";
import {
    ConsoleButton,
    Crumbs,
    Facts,
    Header,
    Inspector,
    InspectorEmpty,
    InspectorHead,
    ListState,
    Cell,
    State,
    Stats,
    Table,
    Tag,
    copy,
    matches,
    useRegion,
    useReportCount,
} from "./parts";
import { formatAgo, formatDuration } from "../../../plugin-api/format";

interface ViewProps {
    paneId: string;
    profile: string;
    active: boolean;
}

/* ── EC2 ─────────────────────────────────────────────── */

export function AwsEc2View({ paneId, profile, active }: ViewProps) {
    const region = useRegion(profile);
    const handle = useResourceEnabled(active, ec2V, paneId, profile);
    const [state, setState] = useState<"all" | "running" | "stopped">("all");
    const [query, setQuery] = useState("");
    const picked = useAws((s) => s.selection[profile]?.ec2);
    const all = handle.data?.instances;
    useReportCount(profile, "ec2", all?.length);
    const running = handle.data?.running ?? 0;
    const rows = (all ?? []).filter(
        (i) =>
            (state === "all" || (state === "running" ? i.state === "running" : i.state !== "running")) &&
            matches(query, i.name, i.id, i.privateIp, i.publicIp, i.instanceType),
    );
    const selected = all?.find((i) => i.id === picked) ?? rows[0];

    return (
        <>
            <div className="aws-main">
                <Header
                    crumbs={<Crumbs profile={profile} region={region} trail={[{ label: "EC2" }]} />}
                    title="Instances"
                    count={all?.length}
                    aside={
                        <Seg
                            label="Instance state"
                            options={[
                                { value: "all", label: "All", count: all?.length ?? 0 },
                                { value: "running", label: "Running", count: running },
                                { value: "stopped", label: "Not running", count: (all?.length ?? 0) - running },
                            ]}
                            value={state}
                            onChange={setState}
                        />
                    }
                    tools={<Filter value={query} onChange={setQuery} placeholder="Filter by name, ID or IP" />}
                    handle={handle}
                />
                <ListState handle={handle} loading="Loading instances…" empty="No instances in this region." count={all?.length} />
                {!!all?.length && (
                    <Table
                        label="Instances"
                        rows={rows}
                        rowKey={(i) => i.id}
                        selected={selected?.id}
                        onSelect={(i) => selectAws(profile, "ec2", i.id)}
                        rowClass={(i) => (i.idle ? "dim" : undefined)}
                        columns={[
                            {
                                header: "Instance",
                                width: "30%",
                                cell: (i) => (
                                    <span className="aws-two">
                                        <span className={i.name ? "aws-name" : "aws-name-dim"}>{i.name ?? "unnamed"}</span>
                                        <span className="aws-sub aws-mono">{i.id}</span>
                                    </span>
                                ),
                            },
                            { header: "State", width: "14%", cell: (i) => <State health={i.health} label={i.stateLabel} /> },
                            { header: "Type", width: "13%", cell: (i) => <Cell>{i.instanceType ?? "—"}</Cell> },
                            { header: "Private IP", width: "14%", cell: (i) => <Cell>{i.privateIp ?? "—"}</Cell> },
                            { header: "Public IP", width: "15%", cell: (i) => <Cell dim={!i.publicIp}>{i.publicIp ?? "—"}</Cell> },
                            { header: "Launched", width: "12%", cell: (i) => <Cell dim>{formatAgo(i.launchedAt)}</Cell> },
                        ]}
                    />
                )}
            </div>
            {selected ? (
                <Inspector>
                    <InspectorHead
                        title={selected.name ?? "unnamed"}
                        badge={<State health={selected.health} label={selected.stateLabel} />}
                        sub={selected.id}
                        actions={
                            <>
                                <ConsoleButton url={selected.consoleUrl} />
                                <button
                                    className="aws-btn aws-icon-btn"
                                    onClick={() => copy(selected.id, "instance ID")}
                                    title="Copy instance ID"
                                    aria-label="Copy instance ID">
                                    <IconCopy size={14} />
                                </button>
                            </>
                        }
                    />
                    <Facts
                        items={[
                            { label: "Type", value: selected.instanceType, copy: true },
                            { label: "Private IP", value: selected.privateIp, copy: true },
                            { label: "Public IP", value: selected.publicIp, copy: true },
                            {
                                label: "Launched",
                                value: selected.launchedAt ? `${formatAgo(selected.launchedAt)} · ${selected.launchedAt.slice(0, 10)}` : null,
                            },
                            { label: "Instance ID", value: selected.id, copy: true },
                        ]}
                    />
                </Inspector>
            ) : (
                <InspectorEmpty text={all ? "No instance selected" : ""} />
            )}
        </>
    );
}

/* ── Lambda ──────────────────────────────────────────── */

function timeout(secs: number | null): string {
    return secs ? formatDuration(secs * 1000) : "—";
}

export function AwsLambdaView({ paneId, profile, active }: ViewProps) {
    const logsFor = useAws((s) => s.lambdaLogs[profile] ?? null);
    if (logsFor) return <LambdaLogs paneId={paneId} profile={profile} active={active} fn={logsFor} />;
    return <LambdaList paneId={paneId} profile={profile} active={active} />;
}

function LambdaList({ paneId, profile, active }: ViewProps) {
    const region = useRegion(profile);
    const handle = useResourceEnabled(active, lambdaV, paneId, profile);
    const [family, setFamily] = useState("all");
    const [query, setQuery] = useState("");
    const picked = useAws((s) => s.selection[profile]?.lambda);
    const all = handle.data?.functions;
    const families = handle.data?.families ?? [];
    useReportCount(profile, "lambda", all?.length);
    const rows = (all ?? []).filter((f) => (family === "all" || f.family === family) && matches(query, f.name, f.runtime, f.handler));
    const selected = all?.find((f) => f.name === picked) ?? rows[0];

    return (
        <>
            <div className="aws-main">
                <Header
                    crumbs={<Crumbs profile={profile} region={region} trail={[{ label: "Lambda" }]} />}
                    title="Functions"
                    count={all?.length}
                    aside={
                        families.length > 1 ? (
                            <Seg
                                label="Runtime"
                                options={[
                                    { value: "all", label: "All", count: all?.length ?? 0 },
                                    ...families.map((f) => ({ value: f.key, label: f.label, count: f.count })),
                                ]}
                                value={family}
                                onChange={setFamily}
                            />
                        ) : undefined
                    }
                    tools={<Filter value={query} onChange={setQuery} placeholder="Filter functions" />}
                    handle={handle}
                />
                <ListState handle={handle} loading="Loading functions…" empty="No functions in this region." count={all?.length} />
                {!!all?.length && (
                    <Table
                        label="Functions"
                        rows={rows}
                        rowKey={(f) => f.name}
                        selected={selected?.name}
                        onSelect={(f) => selectAws(profile, "lambda", f.name)}
                        onOpen={(f) => setLambdaLogs(profile, f.name)}
                        columns={[
                            { header: "Function", width: "33%", cell: (f) => <span className="aws-name">{f.name}</span> },
                            {
                                header: "Runtime",
                                width: "24%",
                                cell: (f) => (
                                    <span className="aws-runtime">
                                        <i className={`rt-${f.family}`} />
                                        <Cell>{f.runtime ?? "—"}</Cell>
                                        {f.deprecated && <Tag tone="warn">deprecated</Tag>}
                                    </span>
                                ),
                            },
                            {
                                header: "Memory",
                                width: "17%",
                                cell: (f) => (
                                    <span className="aws-bar-cell">
                                        <Cell>{f.memoryLabel}</Cell>
                                        <span className="aws-mini">
                                            <b style={{ width: `${f.memoryRatio * 100}%` }} />
                                        </span>
                                    </span>
                                ),
                            },
                            {
                                header: "Timeout",
                                width: "11%",
                                align: "right",
                                cell: (f) => <Cell>{timeout(f.timeoutSecs)}</Cell>,
                            },
                            { header: "Modified", width: "13%", align: "right", cell: (f) => <Cell dim>{formatAgo(f.modifiedAt)}</Cell> },
                        ]}
                    />
                )}
            </div>
            {selected ? <LambdaInspector profile={profile} fn={selected} /> : <InspectorEmpty text={all ? "No function selected" : ""} />}
        </>
    );
}

function LambdaInspector({ profile, fn }: { profile: string; fn: AwsLambdaFunction }) {
    return (
        <Inspector>
            <InspectorHead
                title={fn.name}
                sub={fn.logGroup}
                actions={
                    <>
                        <button className="aws-btn primary" onClick={() => setLambdaLogs(profile, fn.name)}>
                            <IconLogs />
                            Tail logs
                        </button>
                        <ConsoleButton url={fn.consoleUrl} />
                    </>
                }
            />
            <Stats
                items={[
                    { label: "Memory", value: fn.memoryLabel },
                    { label: "Timeout", value: timeout(fn.timeoutSecs) },
                ]}
            />
            <Facts
                items={[
                    { label: "Runtime", value: fn.runtime },
                    { label: "Handler", value: fn.handler, copy: true },
                    { label: "Modified", value: fn.modifiedAt ? `${formatAgo(fn.modifiedAt)} · ${fn.modifiedAt.slice(0, 10)}` : null },
                    { label: "Log group", value: fn.logGroup, copy: true },
                ]}
            />
        </Inspector>
    );
}

function LambdaLogs({ profile, active, fn }: ViewProps & { fn: string }) {
    const region = useRegion(profile);
    const back = () => setLambdaLogs(profile, null);
    return (
        <div className="aws-main">
            <Header
                crumbs={
                    <Crumbs
                        profile={profile}
                        region={region}
                        trail={[
                            { label: "Lambda", onClick: back },
                            { label: "Functions", onClick: back },
                        ]}
                    />
                }
                title={fn}
                tools={
                    <ConsoleButton
                        url={`https://${region}.console.aws.amazon.com/lambda/home?region=${region}#/functions/${encodeURIComponent(fn)}`}
                    />
                }
            />
            <AwsLogTailView key={fn} profile={profile} logGroup={`/aws/lambda/${fn}`} active={active} />
        </div>
    );
}

/* ── SQS ─────────────────────────────────────────────── */

function Count({ n, tone }: { n: number | null; tone?: "warn" }) {
    return <span className={`aws-num${n ? "" : " zero"}${n && tone ? ` ${tone}` : ""}`}>{n === null ? "—" : n.toLocaleString()}</span>;
}

export function AwsSqsView({ paneId, profile, active }: ViewProps) {
    const region = useRegion(profile);
    const handle = useResourceEnabled(active, sqsV, paneId, profile);
    const [kind, setKind] = useState<"all" | "dlq">("all");
    const [query, setQuery] = useState("");
    const picked = useAws((s) => s.selection[profile]?.sqs);
    const all = handle.data?.queues;
    useReportCount(profile, "sqs", all?.length);
    const dlqs = handle.data?.deadLetters ?? 0;
    const rows = (all ?? []).filter((q) => (kind === "all" || q.deadLetter) && matches(query, q.name));
    const selected = all?.find((q) => q.name === picked) ?? rows[0];

    return (
        <>
            <div className="aws-main">
                <Header
                    crumbs={<Crumbs profile={profile} region={region} trail={[{ label: "SQS" }]} />}
                    title="Queues"
                    count={all?.length}
                    aside={
                        dlqs ? (
                            <Seg
                                label="Queue kind"
                                options={[
                                    { value: "all", label: "All", count: all?.length ?? 0 },
                                    { value: "dlq", label: "Dead-letter", count: dlqs },
                                ]}
                                value={kind}
                                onChange={setKind}
                            />
                        ) : undefined
                    }
                    tools={<Filter value={query} onChange={setQuery} placeholder="Filter queues" />}
                    handle={handle}
                />
                <ListState handle={handle} loading="Loading queues and their counts…" empty="No queues in this region." count={all?.length} />
                {!!all?.length && (
                    <Table
                        label="Queues"
                        rows={rows}
                        rowKey={(q) => q.url}
                        selected={selected?.url}
                        onSelect={(q) => selectAws(profile, "sqs", q.name)}
                        columns={[
                            {
                                header: "Queue",
                                width: "38%",
                                cell: (q) => (
                                    <span className="aws-name-row">
                                        <span className="aws-name">{q.name}</span>
                                        {q.fifo && <Tag>fifo</Tag>}
                                        {q.deadLetter && <Tag tone={q.needsLook ? "warn" : undefined}>dead-letter</Tag>}
                                    </span>
                                ),
                            },
                            {
                                header: "Available",
                                width: "22%",
                                cell: (q) => (
                                    <span className="aws-bar-cell">
                                        <span className="aws-bar-num">
                                            <Count n={q.available} tone={q.needsLook ? "warn" : undefined} />
                                        </span>
                                        <span className={`aws-mini wide${q.needsLook ? " warn" : ""}`}>
                                            <b style={{ width: `${q.ratio * 100}%` }} />
                                        </span>
                                    </span>
                                ),
                            },
                            { header: "In flight", width: "11%", align: "right", cell: (q) => <Count n={q.inFlight} /> },
                            { header: "Delayed", width: "11%", align: "right", cell: (q) => <Count n={q.delayed} /> },
                            {
                                header: "",
                                width: "15%",
                                cell: (q) => (q.needsLook ? <State health="warn" label="Needs a look" /> : null),
                            },
                        ]}
                    />
                )}
            </div>
            {selected ? (
                <Inspector>
                    <InspectorHead
                        title={selected.name}
                        badge={selected.fifo ? <Tag>fifo</Tag> : undefined}
                        sub={selected.url}
                        actions={
                            <>
                                <ConsoleButton url={selected.consoleUrl} />
                                <button
                                    className="aws-btn aws-icon-btn"
                                    onClick={() => copy(selected.url, "queue URL")}
                                    title="Copy queue URL"
                                    aria-label="Copy queue URL">
                                    <IconCopy size={14} />
                                </button>
                            </>
                        }
                    />
                    <Stats
                        items={[
                            {
                                label: "Available",
                                value: selected.available?.toLocaleString() ?? "—",
                                tone: selected.needsLook ? "warn" : selected.available ? undefined : "zero",
                            },
                            {
                                label: "In flight",
                                value: selected.inFlight?.toLocaleString() ?? "—",
                                tone: selected.inFlight ? undefined : "zero",
                            },
                            {
                                label: "Delayed",
                                value: selected.delayed?.toLocaleString() ?? "—",
                                tone: selected.delayed ? undefined : "zero",
                            },
                        ]}
                    />
                    <Facts
                        items={[
                            { label: "URL", value: selected.url, copy: true },
                            { label: "Type", value: selected.fifo ? "FIFO" : "Standard" },
                            { label: "Role", value: selected.deadLetter ? "Dead-letter queue" : "Queue" },
                        ]}
                    />
                </Inspector>
            ) : (
                <InspectorEmpty text={all ? "No queue selected" : ""} />
            )}
        </>
    );
}

/* ── S3 ──────────────────────────────────────────────── */

export function AwsS3View({ paneId, profile, active }: ViewProps) {
    const region = useRegion(profile);
    const handle = useResourceEnabled(active, s3V, paneId, profile);
    const [query, setQuery] = useState("");
    const picked = useAws((s) => s.selection[profile]?.s3);
    const all = handle.data?.buckets;
    useReportCount(profile, "s3", all?.length);
    const rows = (all ?? []).filter((b) => matches(query, b.name));
    const selected: AwsS3Bucket | undefined = all?.find((b) => b.name === picked) ?? rows[0];

    return (
        <>
            <div className="aws-main">
                <Header
                    crumbs={<Crumbs profile={profile} region={region} trail={[{ label: "S3" }]} />}
                    title="Buckets"
                    count={all?.length}
                    tools={<Filter value={query} onChange={setQuery} placeholder="Filter buckets" />}
                    handle={handle}
                />
                <ListState handle={handle} loading="Loading buckets…" empty="This account has no buckets." count={all?.length} />
                {!!all?.length && (
                    <Table
                        label="Buckets"
                        rows={rows}
                        rowKey={(b) => b.name}
                        selected={selected?.name}
                        onSelect={(b) => selectAws(profile, "s3", b.name)}
                        columns={[
                            { header: "Bucket", width: "62%", cell: (b) => <span className="aws-name">{b.name}</span> },
                            { header: "Created", width: "20%", cell: (b) => <Cell>{b.createdAt?.slice(0, 10) ?? "—"}</Cell> },
                            { header: "Age", width: "16%", align: "right", cell: (b) => <Cell dim>{formatAgo(b.createdAt)}</Cell> },
                        ]}
                    />
                )}
            </div>
            {selected ? (
                <Inspector>
                    <InspectorHead
                        title={selected.name}
                        sub={selected.arn}
                        actions={
                            <>
                                <ConsoleButton url={selected.consoleUrl} />
                                <button
                                    className="aws-btn aws-icon-btn"
                                    onClick={() => copy(selected.uri, "S3 URI")}
                                    title="Copy s3:// URI"
                                    aria-label="Copy S3 URI">
                                    <IconCopy size={14} />
                                </button>
                            </>
                        }
                    />
                    <Facts
                        items={[
                            { label: "URI", value: selected.uri, copy: true },
                            { label: "ARN", value: selected.arn, copy: true },
                            {
                                label: "Created",
                                value: selected.createdAt ? `${selected.createdAt.slice(0, 10)} · ${formatAgo(selected.createdAt)}` : null,
                            },
                        ]}
                    />
                </Inspector>
            ) : (
                <InspectorEmpty text={all ? "No bucket selected" : ""} />
            )}
        </>
    );
}
