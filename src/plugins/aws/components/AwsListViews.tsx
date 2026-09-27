import { useState } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { IconCopy } from "../../../plugin-api/ui";
import type { Ec2Instance, LambdaFn, S3Bucket, SqsQueue } from "../api";
import { ec2InstancesR, lambdaFnsR, s3BucketsR, sqsQueuesR } from "../resources";
import { selectAws, setLambdaLogs, useAws } from "../state";
import { AwsLogTailView } from "./AwsLogTailView";
import { IconLogs } from "./icons";
import {
    ConsoleButton,
    Crumbs,
    Facts,
    Filter,
    Header,
    Inspector,
    InspectorEmpty,
    InspectorHead,
    ListState,
    Cell,
    Seg,
    State,
    Stats,
    Table,
    Tag,
    copy,
    matches,
    relative,
    useRegion,
    useReportCount,
    type Health,
} from "./parts";

interface ViewProps {
    profile: string;
    active: boolean;
}

/* ── EC2 ─────────────────────────────────────────────── */

const EC2_STATE: Record<string, [Health, string]> = {
    running: ["ok", "Running"],
    pending: ["warn", "Starting"],
    stopping: ["warn", "Stopping"],
    "shutting-down": ["warn", "Shutting down"],
    stopped: ["off", "Stopped"],
    terminated: ["off", "Terminated"],
};

function ec2State(i: Ec2Instance): [Health, string] {
    return EC2_STATE[i.state ?? ""] ?? ["off", i.state ?? "Unknown"];
}

export function AwsEc2View({ profile, active }: ViewProps) {
    const region = useRegion(profile);
    const handle = useResourceEnabled(active, ec2InstancesR, profile);
    const [state, setState] = useState<"all" | "running" | "stopped">("all");
    const [query, setQuery] = useState("");
    const picked = useAws((s) => s.selection[profile]?.ec2);
    const all = handle.data;
    useReportCount(profile, "ec2", all?.length);
    const running = all?.filter((i) => i.state === "running").length ?? 0;
    const rows = (all ?? []).filter(
        (i) =>
            (state === "all" || (state === "running" ? i.state === "running" : i.state !== "running")) &&
            matches(query, i.name, i.instance_id, i.private_ip, i.public_ip, i.instance_type),
    );
    const selected = all?.find((i) => i.instance_id === picked) ?? rows[0];

    return (
        <>
            <div className="aws-main">
                <Header
                    crumbs={<Crumbs profile={profile} region={region} trail={[{ label: "EC2" }]} />}
                    title="Instances"
                    count={all?.length}
                    aside={
                        <Seg
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
                        rowKey={(i) => i.instance_id}
                        selected={selected?.instance_id}
                        onSelect={(i) => selectAws(profile, "ec2", i.instance_id)}
                        rowClass={(i) => (i.state === "stopped" || i.state === "terminated" ? "dim" : undefined)}
                        columns={[
                            {
                                header: "Instance",
                                width: "30%",
                                cell: (i) => (
                                    <span className="aws-two">
                                        <span className={i.name ? "aws-name" : "aws-name-dim"}>{i.name ?? "unnamed"}</span>
                                        <span className="aws-sub aws-mono">{i.instance_id}</span>
                                    </span>
                                ),
                            },
                            { header: "State", width: "14%", cell: (i) => <State health={ec2State(i)[0]} label={ec2State(i)[1]} /> },
                            { header: "Type", width: "13%", cell: (i) => <Cell>{i.instance_type ?? "—"}</Cell> },
                            { header: "Private IP", width: "14%", cell: (i) => <Cell>{i.private_ip ?? "—"}</Cell> },
                            { header: "Public IP", width: "15%", cell: (i) => <Cell dim={!i.public_ip}>{i.public_ip ?? "—"}</Cell> },
                            { header: "Launched", width: "12%", cell: (i) => <Cell dim>{relative(i.launch_time)}</Cell> },
                        ]}
                    />
                )}
            </div>
            {selected ? (
                <Inspector>
                    <InspectorHead
                        title={selected.name ?? "unnamed"}
                        badge={<State health={ec2State(selected)[0]} label={ec2State(selected)[1]} />}
                        sub={selected.instance_id}
                        actions={
                            <>
                                <ConsoleButton
                                    url={`https://${region}.console.aws.amazon.com/ec2/home?region=${region}#InstanceDetails:instanceId=${selected.instance_id}`}
                                />
                                <button
                                    className="aws-btn aws-icon-btn"
                                    onClick={() => copy(selected.instance_id, "instance ID")}
                                    title="Copy instance ID"
                                    aria-label="Copy instance ID">
                                    <IconCopy size={14} />
                                </button>
                            </>
                        }
                    />
                    <Facts
                        items={[
                            { label: "Type", value: selected.instance_type, copy: true },
                            { label: "Private IP", value: selected.private_ip, copy: true },
                            { label: "Public IP", value: selected.public_ip, copy: true },
                            {
                                label: "Launched",
                                value: selected.launch_time ? `${relative(selected.launch_time)} · ${selected.launch_time.slice(0, 10)}` : null,
                            },
                            { label: "Instance ID", value: selected.instance_id, copy: true },
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

const DEPRECATED_RUNTIMES = new Set([
    "python3.7",
    "python3.8",
    "python3.9",
    "nodejs12.x",
    "nodejs14.x",
    "nodejs16.x",
    "nodejs18.x",
    "go1.x",
    "java8",
    "dotnet6",
    "ruby2.7",
]);

function runtimeFamily(runtime: string | null): string {
    if (!runtime) return "other";
    const family = ["nodejs", "python", "java", "go", "dotnet", "ruby", "provided"].find((f) => runtime.startsWith(f));
    return family ?? "other";
}

function memory(mb: number | null): string {
    if (!mb) return "—";
    return mb >= 1024 ? `${+(mb / 1024).toFixed(1)} GB` : `${mb} MB`;
}

function duration(seconds: number | null): string {
    if (!seconds) return "—";
    return seconds >= 60 && seconds % 60 === 0 ? `${seconds / 60} min` : `${seconds} s`;
}

export function AwsLambdaView({ profile, active }: ViewProps) {
    const logsFor = useAws((s) => s.lambdaLogs[profile] ?? null);
    if (logsFor) return <LambdaLogs profile={profile} active={active} fn={logsFor} />;
    return <LambdaList profile={profile} active={active} />;
}

function LambdaList({ profile, active }: ViewProps) {
    const region = useRegion(profile);
    const handle = useResourceEnabled(active, lambdaFnsR, profile);
    const [family, setFamily] = useState("all");
    const [query, setQuery] = useState("");
    const picked = useAws((s) => s.selection[profile]?.lambda);
    const all = handle.data;
    useReportCount(profile, "lambda", all?.length);

    const families = new Map<string, number>();
    for (const f of all ?? []) families.set(runtimeFamily(f.runtime), (families.get(runtimeFamily(f.runtime)) ?? 0) + 1);
    const familyLabel: Record<string, string> = {
        nodejs: "Node",
        python: "Python",
        java: "Java",
        go: "Go",
        dotnet: ".NET",
        ruby: "Ruby",
        provided: "Custom",
        other: "Other",
    };
    const rows = (all ?? []).filter((f) => (family === "all" || runtimeFamily(f.runtime) === family) && matches(query, f.name, f.runtime, f.handler));
    const selected = all?.find((f) => f.name === picked) ?? rows[0];
    const maxMemory = Math.max(1024, ...(all ?? []).map((f) => f.memory_size ?? 0));

    return (
        <>
            <div className="aws-main">
                <Header
                    crumbs={<Crumbs profile={profile} region={region} trail={[{ label: "Lambda" }]} />}
                    title="Functions"
                    count={all?.length}
                    aside={
                        families.size > 1 ? (
                            <Seg
                                options={[
                                    { value: "all", label: "All", count: all?.length ?? 0 },
                                    ...[...families].sort((a, b) => b[1] - a[1]).map(([f, n]) => ({ value: f, label: familyLabel[f], count: n })),
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
                                        <i className={`rt-${runtimeFamily(f.runtime)}`} />
                                        <Cell>{f.runtime ?? "—"}</Cell>
                                        {f.runtime && DEPRECATED_RUNTIMES.has(f.runtime) && <Tag tone="warn">deprecated</Tag>}
                                    </span>
                                ),
                            },
                            {
                                header: "Memory",
                                width: "17%",
                                cell: (f) => (
                                    <span className="aws-bar-cell">
                                        <Cell>{memory(f.memory_size)}</Cell>
                                        <span className="aws-mini">
                                            <b style={{ width: `${((f.memory_size ?? 0) / maxMemory) * 100}%` }} />
                                        </span>
                                    </span>
                                ),
                            },
                            { header: "Timeout", width: "11%", align: "right", cell: (f) => <Cell>{duration(f.timeout)}</Cell> },
                            { header: "Modified", width: "13%", align: "right", cell: (f) => <Cell dim>{relative(f.last_modified)}</Cell> },
                        ]}
                    />
                )}
            </div>
            {selected ? (
                <LambdaInspector profile={profile} fn={selected} region={region} />
            ) : (
                <InspectorEmpty text={all ? "No function selected" : ""} />
            )}
        </>
    );
}

function LambdaInspector({ profile, fn, region }: { profile: string; fn: LambdaFn; region: string }) {
    return (
        <Inspector>
            <InspectorHead
                title={fn.name}
                sub={`/aws/lambda/${fn.name}`}
                actions={
                    <>
                        <button className="aws-btn primary" onClick={() => setLambdaLogs(profile, fn.name)}>
                            <IconLogs />
                            Tail logs
                        </button>
                        <ConsoleButton
                            url={`https://${region}.console.aws.amazon.com/lambda/home?region=${region}#/functions/${encodeURIComponent(fn.name)}`}
                        />
                    </>
                }
            />
            <Stats
                items={[
                    { label: "Memory", value: memory(fn.memory_size) },
                    { label: "Timeout", value: duration(fn.timeout) },
                ]}
            />
            <Facts
                items={[
                    { label: "Runtime", value: fn.runtime },
                    { label: "Handler", value: fn.handler, copy: true },
                    { label: "Modified", value: fn.last_modified ? `${relative(fn.last_modified)} · ${fn.last_modified.slice(0, 10)}` : null },
                    { label: "Log group", value: `/aws/lambda/${fn.name}`, copy: true },
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

const count = (value: string | null) => {
    const n = Number(value);
    return value !== null && Number.isFinite(n) ? n : null;
};
const isDeadLetter = (q: SqsQueue) => /(?:-|_)(?:dlq|dead-?letter)(?:\.fifo)?$/i.test(q.name);

function Count({ n, tone }: { n: number | null; tone?: "warn" }) {
    return <span className={`aws-num${n ? "" : " zero"}${n && tone ? ` ${tone}` : ""}`}>{n === null ? "—" : n.toLocaleString()}</span>;
}

export function AwsSqsView({ profile, active }: ViewProps) {
    const region = useRegion(profile);
    const handle = useResourceEnabled(active, sqsQueuesR, profile);
    const [kind, setKind] = useState<"all" | "dlq">("all");
    const [query, setQuery] = useState("");
    const picked = useAws((s) => s.selection[profile]?.sqs);
    const all = handle.data;
    useReportCount(profile, "sqs", all?.length);
    const dlqs = all?.filter(isDeadLetter).length ?? 0;
    const rows = (all ?? []).filter((q) => (kind === "all" || isDeadLetter(q)) && matches(query, q.name));
    const selected = all?.find((q) => q.name === picked) ?? rows[0];
    const most = Math.max(1, ...(all ?? []).map((q) => count(q.messages) ?? 0));

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
                                        {q.name.endsWith(".fifo") && <Tag>fifo</Tag>}
                                        {isDeadLetter(q) && <Tag tone={count(q.messages) ? "warn" : undefined}>dead-letter</Tag>}
                                    </span>
                                ),
                            },
                            {
                                header: "Available",
                                width: "22%",
                                cell: (q) => {
                                    const n = count(q.messages);
                                    const alarm = isDeadLetter(q) && !!n;
                                    return (
                                        <span className="aws-bar-cell">
                                            <span className="aws-bar-num">
                                                <Count n={n} tone={alarm ? "warn" : undefined} />
                                            </span>
                                            <span className={`aws-mini wide${alarm ? " warn" : ""}`}>
                                                <b style={{ width: `${n ? Math.max(3, Math.sqrt(n / most) * 100) : 0}%` }} />
                                            </span>
                                        </span>
                                    );
                                },
                            },
                            { header: "In flight", width: "11%", align: "right", cell: (q) => <Count n={count(q.in_flight)} /> },
                            { header: "Delayed", width: "11%", align: "right", cell: (q) => <Count n={count(q.delayed)} /> },
                            {
                                header: "",
                                width: "15%",
                                cell: (q) => (isDeadLetter(q) && count(q.messages) ? <State health="warn" label="Needs a look" /> : null),
                            },
                        ]}
                    />
                )}
            </div>
            {selected ? (
                <Inspector>
                    <InspectorHead
                        title={selected.name}
                        badge={selected.name.endsWith(".fifo") ? <Tag>fifo</Tag> : undefined}
                        sub={selected.url}
                        actions={
                            <>
                                <ConsoleButton
                                    url={`https://${region}.console.aws.amazon.com/sqs/v3/home?region=${region}#/queues/${encodeURIComponent(selected.url)}`}
                                />
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
                                value: count(selected.messages)?.toLocaleString() ?? "—",
                                tone: isDeadLetter(selected) && count(selected.messages) ? "warn" : count(selected.messages) ? undefined : "zero",
                            },
                            {
                                label: "In flight",
                                value: count(selected.in_flight)?.toLocaleString() ?? "—",
                                tone: count(selected.in_flight) ? undefined : "zero",
                            },
                            {
                                label: "Delayed",
                                value: count(selected.delayed)?.toLocaleString() ?? "—",
                                tone: count(selected.delayed) ? undefined : "zero",
                            },
                        ]}
                    />
                    <Facts
                        items={[
                            { label: "URL", value: selected.url, copy: true },
                            { label: "Type", value: selected.name.endsWith(".fifo") ? "FIFO" : "Standard" },
                            { label: "Role", value: isDeadLetter(selected) ? "Dead-letter queue" : "Queue" },
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

export function AwsS3View({ profile, active }: ViewProps) {
    const region = useRegion(profile);
    const handle = useResourceEnabled(active, s3BucketsR, profile);
    const [query, setQuery] = useState("");
    const picked = useAws((s) => s.selection[profile]?.s3);
    const all = handle.data;
    useReportCount(profile, "s3", all?.length);
    const rows = (all ?? []).filter((b) => matches(query, b.name));
    const selected: S3Bucket | undefined = all?.find((b) => b.name === picked) ?? rows[0];

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
                            { header: "Created", width: "20%", cell: (b) => <Cell>{b.created_at?.slice(0, 10) ?? "—"}</Cell> },
                            { header: "Age", width: "16%", align: "right", cell: (b) => <Cell dim>{relative(b.created_at)}</Cell> },
                        ]}
                    />
                )}
            </div>
            {selected ? (
                <Inspector>
                    <InspectorHead
                        title={selected.name}
                        sub={`arn:aws:s3:::${selected.name}`}
                        actions={
                            <>
                                <ConsoleButton url={`https://s3.console.aws.amazon.com/s3/buckets/${encodeURIComponent(selected.name)}`} />
                                <button
                                    className="aws-btn aws-icon-btn"
                                    onClick={() => copy(`s3://${selected.name}`, "S3 URI")}
                                    title="Copy s3:// URI"
                                    aria-label="Copy S3 URI">
                                    <IconCopy size={14} />
                                </button>
                            </>
                        }
                    />
                    <Facts
                        items={[
                            { label: "URI", value: `s3://${selected.name}`, copy: true },
                            { label: "ARN", value: `arn:aws:s3:::${selected.name}`, copy: true },
                            {
                                label: "Created",
                                value: selected.created_at ? `${selected.created_at.slice(0, 10)} · ${relative(selected.created_at)}` : null,
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
