import { useState } from "react";
import { reportError } from "../../../plugin-api/host";
import { useResourceEnabled, type ResourceHandle } from "../../../plugin-api/resources";
import { Filter, IconChevron, IconClose, IconCopy, Seg } from "../../../plugin-api/ui";
import { awsApi } from "../api";
import { ecsClustersV, ecsServiceLogConfigR, ecsServicesV, ecsTasksV } from "../resources";
import { selectAws, setEcsLevel, useAws, type EcsLevel } from "../state";
import { AwsLogTailView } from "./AwsLogTailView";
import type { AwsEcsClusterRow, AwsEcsServiceRow, AwsEcsTaskRow, AwsEcsTasks } from "../types.generated";
import { IconLogs, IconTasks } from "./icons";
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
    Section,
    State,
    Stats,
    Table,
    TaskMeter,
    copy,
    matches,
    useRegion,
    useReportCount,
} from "./parts";
import { formatAgo } from "../../../plugin-api/format";

interface ViewProps {
    paneId: string;
    profile: string;
    active: boolean;
}

const DEFAULT_LEVEL: EcsLevel = { kind: "clusters" };

function ClusterName({ cluster }: { cluster: AwsEcsClusterRow }) {
    return (
        <span className="aws-name">
            {cluster.shortName}
            {cluster.shortName !== cluster.name && <span className="aws-name-dim">-cluster</span>}
        </span>
    );
}

function EnvChip({ cluster }: { cluster: Pick<AwsEcsClusterRow, "envLabel" | "production"> }) {
    return <span className={`aws-env${cluster.production ? " prod" : ""}`}>{cluster.envLabel}</span>;
}

export function AwsEcsView({ paneId, profile, active }: ViewProps) {
    const level = useAws((s) => s.ecsViews[profile] ?? DEFAULT_LEVEL);
    const setLevel = (l: EcsLevel) => setEcsLevel(profile, l);

    if (level.kind === "clusters")
        return <Clusters paneId={paneId} profile={profile} active={active} onOpen={(cluster) => setLevel({ kind: "services", cluster })} />;
    if (level.kind === "services")
        return (
            <Services
                paneId={paneId}
                profile={profile}
                active={active}
                cluster={level.cluster}
                onBack={() => setLevel({ kind: "clusters" })}
                onOpen={(service, tab) => setLevel({ kind: "service", cluster: level.cluster, service, tab })}
                onOpenTask={(service, taskFilter) => setLevel({ kind: "service", cluster: level.cluster, service, tab: "logs", taskFilter })}
            />
        );
    return <ServicePage paneId={paneId} profile={profile} active={active} level={level} setLevel={setLevel} />;
}

function Clusters({ paneId, profile, active, onOpen }: ViewProps & { onOpen: (cluster: string) => void }) {
    const region = useRegion(profile);
    const handle = useResourceEnabled(active, ecsClustersV, paneId, profile);
    const [env, setEnv] = useState("all");
    const [query, setQuery] = useState("");
    const picked = useAws((s) => s.selection[profile]?.cluster);
    const all = handle.data?.clusters;
    const envs = handle.data?.envs ?? [];
    useReportCount(profile, "ecs", all?.length);
    const envOptions = [
        { value: "all", label: "All", count: all?.length ?? 0 },
        ...envs.map((e) => ({ value: e.key, label: e.label, count: e.count })),
    ];
    const rows = (all ?? []).filter((c) => (env === "all" || c.env === env) && matches(query, c.name));
    const selected = all?.find((c) => c.name === picked) ?? rows[0];

    return (
        <>
            <div className="aws-main">
                <Header
                    crumbs={<Crumbs profile={profile} region={region} trail={[{ label: "ECS" }]} />}
                    title="Clusters"
                    count={all?.length}
                    aside={envs.length > 1 ? <Seg label="Environment" options={envOptions} value={env} onChange={setEnv} /> : undefined}
                    tools={<Filter value={query} onChange={setQuery} placeholder="Filter" />}
                    handle={handle}
                />
                <ListState handle={handle} loading="Loading clusters…" empty="No clusters in this region." count={all?.length} />
                {!!all?.length && (
                    <Table
                        label="Clusters"
                        rows={rows}
                        rowKey={(c) => c.name}
                        selected={selected?.name}
                        onSelect={(c) => selectAws(profile, "cluster", c.name)}
                        onOpen={(c) => onOpen(c.name)}
                        columns={[
                            { header: "Cluster", width: "36%", cell: (c) => <ClusterName cluster={c} /> },
                            { header: "Environment", width: "13%", cell: (c) => <EnvChip cluster={c} /> },
                            { header: "Services", width: "8%", align: "right", cell: (c) => <Num n={c.services} /> },
                            {
                                header: "Tasks",
                                width: "8%",
                                align: "right",
                                cell: (c) => (
                                    <>
                                        <Num n={c.running} />
                                        {!!c.pending && <span className="aws-num warn"> +{c.pending}</span>}
                                    </>
                                ),
                            },
                            {
                                header: "Capacity",
                                width: "22%",
                                cell: (c) => <TaskMeter running={c.running} pending={c.pending} target={c.services} />,
                            },
                            { header: "Status", width: "13%", cell: (c) => <State health={c.health} /> },
                        ]}
                    />
                )}
            </div>
            {selected ? (
                <ClusterInspector paneId={paneId} profile={profile} active={active} cluster={selected} onOpen={onOpen} />
            ) : (
                <InspectorEmpty text={all ? "No cluster selected" : ""} />
            )}
        </>
    );
}

function ClusterInspector({
    paneId,
    profile,
    active,
    cluster,
    onOpen,
}: ViewProps & { cluster: AwsEcsClusterRow; onOpen: (cluster: string) => void }) {
    const services = useResourceEnabled(active, ecsServicesV, paneId, profile, cluster.name);
    const { running, pending } = cluster;
    const list = services.data?.services ?? [];
    const openService = (service: string) => setEcsLevel(profile, { kind: "service", cluster: cluster.name, service, tab: "logs" });
    return (
        <Inspector>
            <InspectorHead
                title={cluster.product}
                badge={<EnvChip cluster={cluster} />}
                sub={cluster.arn}
                actions={
                    <>
                        <button className="aws-btn primary" onClick={() => onOpen(cluster.name)}>
                            Open cluster
                        </button>
                        <ConsoleButton url={cluster.consoleUrl} />
                        <button className="aws-btn aws-icon-btn" onClick={() => copy(cluster.arn, "ARN")} title="Copy ARN" aria-label="Copy ARN">
                            <IconCopy size={14} />
                        </button>
                    </>
                }
            />
            <Stats
                items={[
                    { label: "Services", value: cluster.services },
                    { label: "Running", value: running },
                    { label: "Pending", value: pending, tone: pending ? "warn" : "zero" },
                ]}
            />
            <Section title="Tasks" meta={`${running}/${Math.max(cluster.services, running)}`}>
                <TaskMeter running={running} pending={pending} target={cluster.services} wrap />
            </Section>
            <Section title="Services" meta={services.data ? list.length : undefined}>
                {!services.data && <div className="aws-insp-note">{services.status === "error" ? services.error : "Loading services…"}</div>}
                {list.slice(0, 10).map((s) => (
                    <button key={s.arn} className="aws-insp-row" onClick={() => openService(s.name)} title="Tail this service's logs">
                        <State health={s.health} label="" />
                        <span className="aws-name aws-grow">{s.name}</span>
                        <span className="aws-num dim">
                            {s.running}/{s.desired}
                        </span>
                        <IconChevron size={12} />
                    </button>
                ))}
                {list.length > 10 && (
                    <button className="aws-insp-row more" onClick={() => onOpen(cluster.name)}>
                        {list.length - 10} more
                    </button>
                )}
            </Section>
        </Inspector>
    );
}

function Num({ n }: { n: number | null | undefined }) {
    return <span className={`aws-num${!n ? " zero" : ""}`}>{n ?? "—"}</span>;
}

function Services({
    paneId,
    profile,
    active,
    cluster,
    onBack,
    onOpen,
    onOpenTask,
}: ViewProps & {
    cluster: string;
    onBack: () => void;
    onOpen: (service: string, tab: "logs" | "tasks") => void;
    onOpenTask: (service: string, filter: { taskId: string; stream: string }) => void;
}) {
    const region = useRegion(profile);
    const handle = useResourceEnabled(active, ecsServicesV, paneId, profile, cluster);
    const clusterRow = useResourceEnabled(active, ecsClustersV, paneId, profile).data?.clusters.find((c) => c.name === cluster);
    const [query, setQuery] = useState("");
    const picked = useAws((s) => s.selection[profile]?.service);
    const all = handle.data?.services;
    const rows = (all ?? []).filter((s) => matches(query, s.name));
    const selected = all?.find((s) => s.name === picked) ?? rows[0];

    return (
        <>
            <div className="aws-main">
                <Header
                    crumbs={
                        <Crumbs
                            profile={profile}
                            region={region}
                            trail={[
                                { label: "ECS", onClick: onBack },
                                { label: "Clusters", onClick: onBack },
                            ]}
                        />
                    }
                    title={
                        <>
                            <span>{cluster}</span>
                            {clusterRow && <EnvChip cluster={clusterRow} />}
                        </>
                    }
                    count={all?.length}
                    tools={<Filter value={query} onChange={setQuery} placeholder="Filter services" />}
                    handle={handle}
                />
                <ListState handle={handle} loading="Loading services…" empty="This cluster runs no services." count={all?.length} />
                {!!all?.length && (
                    <Table
                        label="Services"
                        rows={rows}
                        rowKey={(s) => s.name}
                        selected={selected?.name}
                        onSelect={(s) => selectAws(profile, "service", s.name)}
                        onOpen={(s) => onOpen(s.name, "logs")}
                        columns={[
                            { header: "Service", width: "32%", cell: (s) => <span className="aws-name">{s.name}</span> },
                            {
                                header: "Running",
                                width: "10%",
                                align: "right",
                                cell: (s) => (
                                    <>
                                        <span className="aws-num">{s.running}</span>
                                        <span className="aws-num dim">/{s.desired}</span>
                                    </>
                                ),
                            },
                            { header: "Pending", width: "9%", align: "right", cell: (s) => <Num n={s.pending} /> },
                            {
                                header: "Tasks",
                                width: "19%",
                                cell: (s) => <TaskMeter running={s.running} pending={s.pending} target={s.desired} />,
                            },
                            {
                                header: "Deployed",
                                width: "13%",
                                cell: (s) => <span className="aws-num dim">{formatAgo(s.deployedAt)}</span>,
                            },
                            {
                                header: "Status",
                                width: "14%",
                                cell: (s) => <State health={s.health} label={s.stateLabel} />,
                            },
                        ]}
                    />
                )}
            </div>
            {selected ? (
                <ServiceInspector
                    paneId={paneId}
                    profile={profile}
                    active={active}
                    cluster={cluster}
                    service={selected}
                    onOpen={(tab) => onOpen(selected.name, tab)}
                    onOpenTask={(filter) => onOpenTask(selected.name, filter)}
                />
            ) : (
                <InspectorEmpty text={all ? "No service selected" : ""} />
            )}
        </>
    );
}

function ServiceInspector({
    paneId,
    profile,
    active,
    cluster,
    service,
    onOpen,
    onOpenTask,
}: ViewProps & {
    cluster: string;
    service: AwsEcsServiceRow;
    onOpen: (tab: "logs" | "tasks") => void;
    onOpenTask: (filter: { taskId: string; stream: string }) => void;
}) {
    const logs = useResourceEnabled(active, ecsServiceLogConfigR, profile, cluster, service.name);
    const tasks = useResourceEnabled(active, ecsTasksV, paneId, profile, cluster, service.name);
    const pending = service.pending;
    const openTask = (t: AwsEcsTaskRow) => {
        void awsApi
            .ecsTaskLogConfig(profile, cluster, t.arn)
            .then((cfg) => onOpenTask({ taskId: t.id, stream: cfg.log_stream }))
            .catch(reportError("task log config"));
    };
    return (
        <Inspector>
            <InspectorHead
                title={service.name}
                sub={service.arn}
                actions={
                    <>
                        <button className="aws-btn primary" onClick={() => onOpen("logs")}>
                            <IconLogs />
                            Tail logs
                        </button>
                        <button className="aws-btn" onClick={() => onOpen("tasks")}>
                            <IconTasks />
                            Tasks
                        </button>
                        <ConsoleButton url={service.consoleUrl} />
                    </>
                }
            />
            <Stats
                items={[
                    { label: "Desired", value: service.desired },
                    { label: "Running", value: service.running },
                    { label: "Pending", value: pending, tone: pending ? "warn" : "zero" },
                ]}
            />
            <Facts
                items={[
                    { label: "Status", value: service.stateLabel },
                    { label: "Deployed", value: formatAgo(service.deployedAt) },
                    { label: "Log group", value: logs.data?.log_group ?? (logs.status === "loading" ? "…" : null), copy: true },
                ]}
            />
            <Section title="Tasks" meta={tasks.data?.tasks.length}>
                {!tasks.data && <div className="aws-insp-note">{tasks.status === "error" ? tasks.error : "Loading tasks…"}</div>}
                {tasks.data?.tasks.length === 0 && <div className="aws-insp-note">No tasks running.</div>}
                {tasks.data?.tasks.slice(0, 8).map((t) => (
                    <button key={t.arn} className="aws-insp-row" onClick={() => openTask(t)} title="Tail this task's logs">
                        <State health={t.health === "ok" ? "ok" : "warn"} label="" />
                        <span className="aws-mono aws-grow">{t.shortId}</span>
                        <span className="aws-num dim">
                            {t.zone ? `${t.zone.slice(-2)} · ` : ""}
                            {formatAgo(t.startedAt)}
                        </span>
                        <IconChevron size={12} />
                    </button>
                ))}
            </Section>
        </Inspector>
    );
}

function ServicePage({
    paneId,
    profile,
    active,
    level,
    setLevel,
}: ViewProps & { level: Extract<EcsLevel, { kind: "service" }>; setLevel: (l: EcsLevel) => void }) {
    const region = useRegion(profile);
    const { cluster, service, tab, taskFilter } = level;
    const services = useResourceEnabled(active, ecsServicesV, paneId, profile, cluster);
    const tasks = useResourceEnabled(active, ecsTasksV, paneId, profile, cluster, service);
    const cfg = useResourceEnabled(active, ecsServiceLogConfigR, profile, cluster, service);
    const info = services.data?.services.find((s) => s.name === service);
    const toClusters = () => setLevel({ kind: "clusters" });

    const lead = (
        <>
            <Seg
                label="Show"
                options={[
                    { value: "logs" as const, label: "Logs" },
                    { value: "tasks" as const, label: "Tasks", count: tasks.data?.tasks.length },
                ]}
                value={tab}
                onChange={(t) => setLevel({ ...level, tab: t })}
            />
            {taskFilter && tab === "logs" && (
                <span className="aws-filter-chip">
                    Task <span className="aws-mono">{taskFilter.taskId.slice(0, 12)}</span>
                    <button onClick={() => setLevel({ kind: "service", cluster, service, tab })} title="Show every task" aria-label="Show every task">
                        <IconClose size={11} />
                    </button>
                </span>
            )}
        </>
    );

    return (
        <div className="aws-main">
            <Header
                crumbs={
                    <Crumbs
                        profile={profile}
                        region={region}
                        trail={[
                            { label: "ECS", onClick: toClusters },
                            { label: "Clusters", onClick: toClusters },
                            { label: cluster.replace(/-cluster$/, ""), onClick: () => setLevel({ kind: "services", cluster }) },
                        ]}
                    />
                }
                title={service}
                aside={
                    info && (
                        <span className="aws-head-meta">
                            <State health={info.health} label={info.stateLabel} />
                            <span className="aws-num dim">
                                {info.running}/{info.desired} tasks · deployed {formatAgo(info.deployedAt)}
                            </span>
                        </span>
                    )
                }
                tools={info && <ConsoleButton url={info.consoleUrl} />}
                handle={tab === "tasks" ? tasks : undefined}
            />
            {tab === "logs" &&
                (cfg.error ? (
                    <div className="aws-note err">{cfg.error}</div>
                ) : !cfg.data ? (
                    <div className="aws-note">Finding the service's log group…</div>
                ) : (
                    <AwsLogTailView
                        key={`${cfg.data.log_group}|${taskFilter?.stream ?? ""}`}
                        profile={profile}
                        logGroup={cfg.data.log_group}
                        logStream={taskFilter?.stream ?? null}
                        active={active}
                        lead={lead}
                    />
                ))}
            {tab === "tasks" && (
                <>
                    <div className="aws-subbar">{lead}</div>
                    <TasksList
                        profile={profile}
                        cluster={cluster}
                        handle={tasks}
                        onPick={(taskId, stream) => setLevel({ kind: "service", cluster, service, tab: "logs", taskFilter: { taskId, stream } })}
                    />
                </>
            )}
        </div>
    );
}

function TasksList({
    profile,
    cluster,
    handle,
    onPick,
}: {
    profile: string;
    cluster: string;
    handle: ResourceHandle<AwsEcsTasks>;
    onPick: (taskId: string, stream: string) => void;
}) {
    const tasks = handle.data?.tasks;
    const open = (t: AwsEcsTaskRow) => {
        void awsApi
            .ecsTaskLogConfig(profile, cluster, t.arn)
            .then((cfg) => onPick(t.id, cfg.log_stream))
            .catch(reportError("task log config"));
    };
    return (
        <>
            <ListState handle={handle} loading="Loading tasks…" empty="No tasks running." count={tasks?.length} />
            {!!tasks?.length && (
                <Table
                    label="Tasks"
                    rows={tasks}
                    rowKey={(t) => t.arn}
                    onOpen={open}
                    columns={[
                        {
                            header: "Task",
                            width: "22%",
                            cell: (t) => (
                                <span className="aws-mono">
                                    <span className="aws-name">{t.shortId}</span>
                                    <span className="aws-name-dim">{t.idRest}…</span>
                                </span>
                            ),
                        },
                        {
                            header: "Status",
                            width: "14%",
                            cell: (t) => <State health={t.health} label={t.statusLabel} />,
                        },
                        {
                            header: "Health",
                            width: "11%",
                            cell: (t) => <span className={`aws-health ${t.check ?? ""}`}>{t.checkLabel}</span>,
                        },
                        { header: "Private IP", width: "13%", cell: (t) => <Cell>{t.privateIp ?? "—"}</Cell> },
                        { header: "Zone", width: "12%", cell: (t) => <Cell>{t.zone ?? "—"}</Cell> },
                        { header: "Size", width: "14%", cell: (t) => <Cell dim>{t.size ?? "—"}</Cell> },
                        { header: "Started", width: "10%", cell: (t) => <Cell dim>{formatAgo(t.startedAt)}</Cell> },
                        {
                            header: "",
                            width: "4%",
                            cell: () => (
                                <span className="aws-row-hint" title="Tail this task's logs">
                                    <IconLogs size={13} />
                                </span>
                            ),
                        },
                    ]}
                />
            )}
        </>
    );
}
