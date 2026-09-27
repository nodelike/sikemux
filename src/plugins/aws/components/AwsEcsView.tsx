import { useState } from "react";
import { reportError } from "../../../plugin-api/host";
import { useResourceEnabled, type ResourceHandle } from "../../../plugin-api/resources";
import { IconChevron, IconClose, IconCopy } from "../../../plugin-api/ui";
import { awsApi, type EcsCluster, type EcsService, type EcsTask } from "../api";
import { ecsClustersR, ecsServiceLogConfigR, ecsServicesR, ecsTasksR } from "../resources";
import { selectAws, setEcsLevel, useAws, type EcsLevel } from "../state";
import { AwsLogTailView } from "./AwsLogTailView";
import { IconLogs, IconTasks } from "./icons";
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
    Section,
    Seg,
    State,
    Stats,
    Table,
    TaskMeter,
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

const DEFAULT_LEVEL: EcsLevel = { kind: "clusters" };
const ENV_ORDER = ["dev", "development", "test", "qa", "uat", "sandbox", "demo", "staging", "stage", "preprod", "prod", "production"];
const PRODUCTION = new Set(["prod", "production"]);

/** Reads `<product>-<env>-cluster`; names that do not follow it land in "other". */
function parseCluster(name: string): { product: string; env: string; label: string } {
    const parts = name.replace(/-cluster$/, "").split("-");
    const last = parts[parts.length - 1];
    if (ENV_ORDER.includes(last)) return { product: parts.length > 1 ? parts.slice(0, -1).join("-") : last, env: last, label: last };
    return { product: parts[0], env: "other", label: parts.slice(1).join("-") || "other" };
}

function clusterHealth(c: EcsCluster): Health {
    if (c.status && c.status !== "ACTIVE") return c.status === "INACTIVE" ? "off" : "fail";
    return (c.tasks_pending ?? 0) > 0 ? "warn" : "ok";
}

function serviceHealth(s: EcsService): Health {
    const desired = s.desired ?? 0;
    const running = s.running ?? 0;
    if ((s.pending ?? 0) > 0) return "warn";
    if (desired === 0 && running === 0) return "off";
    if (running === desired) return "ok";
    return "fail";
}

const SERVICE_STATE: Record<Health, string> = { ok: "Steady", warn: "Deploying", fail: "Degraded", off: "Scaled to 0" };

function ClusterName({ name }: { name: string }) {
    const cut = name.endsWith("-cluster") ? name.length - "-cluster".length : name.length;
    return (
        <span className="aws-name">
            {name.slice(0, cut)}
            {cut < name.length && <span className="aws-name-dim">-cluster</span>}
        </span>
    );
}

function EnvChip({ name }: { name: string }) {
    const { env, label } = parseCluster(name);
    return <span className={`aws-env${PRODUCTION.has(env) ? " prod" : ""}`}>{label}</span>;
}

function consoleBase(region: string) {
    return `https://${region}.console.aws.amazon.com/ecs/v2/clusters`;
}

export function AwsEcsView({ profile, active }: ViewProps) {
    const level = useAws((s) => s.ecsViews[profile] ?? DEFAULT_LEVEL);
    const setLevel = (l: EcsLevel) => setEcsLevel(profile, l);

    if (level.kind === "clusters")
        return <Clusters profile={profile} active={active} onOpen={(cluster) => setLevel({ kind: "services", cluster })} />;
    if (level.kind === "services")
        return (
            <Services
                profile={profile}
                active={active}
                cluster={level.cluster}
                onBack={() => setLevel({ kind: "clusters" })}
                onOpen={(service, tab) => setLevel({ kind: "service", cluster: level.cluster, service, tab })}
                onOpenTask={(service, taskFilter) => setLevel({ kind: "service", cluster: level.cluster, service, tab: "logs", taskFilter })}
            />
        );
    return <ServicePage profile={profile} active={active} level={level} setLevel={setLevel} />;
}

function Clusters({ profile, active, onOpen }: ViewProps & { onOpen: (cluster: string) => void }) {
    const region = useRegion(profile);
    const handle = useResourceEnabled(active, ecsClustersR, profile);
    const [env, setEnv] = useState("all");
    const [query, setQuery] = useState("");
    const picked = useAws((s) => s.selection[profile]?.cluster);
    const all = handle.data;
    useReportCount(profile, "ecs", all?.length);

    const envs = new Map<string, number>();
    for (const c of all ?? []) {
        const e = parseCluster(c.name).env;
        envs.set(e, (envs.get(e) ?? 0) + 1);
    }
    const envOptions = [
        { value: "all", label: "All", count: all?.length ?? 0 },
        ...[...envs.keys()]
            .sort((a, b) => (ENV_ORDER.indexOf(a) + 1 || 99) - (ENV_ORDER.indexOf(b) + 1 || 99))
            .map((e) => ({ value: e, label: e === "other" ? "Other" : e[0].toUpperCase() + e.slice(1), count: envs.get(e) })),
    ];
    const rows = (all ?? []).filter((c) => (env === "all" || parseCluster(c.name).env === env) && matches(query, c.name));
    const selected = all?.find((c) => c.name === picked) ?? rows[0];

    return (
        <>
            <div className="aws-main">
                <Header
                    crumbs={<Crumbs profile={profile} region={region} trail={[{ label: "ECS" }]} />}
                    title="Clusters"
                    count={all?.length}
                    aside={envs.size > 1 ? <Seg options={envOptions} value={env} onChange={setEnv} /> : undefined}
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
                            { header: "Cluster", width: "36%", cell: (c) => <ClusterName name={c.name} /> },
                            { header: "Environment", width: "13%", cell: (c) => <EnvChip name={c.name} /> },
                            { header: "Services", width: "8%", align: "right", cell: (c) => <Num n={c.services_count} /> },
                            {
                                header: "Tasks",
                                width: "8%",
                                align: "right",
                                cell: (c) => (
                                    <>
                                        <Num n={c.tasks_running} />
                                        {!!c.tasks_pending && <span className="aws-num warn"> +{c.tasks_pending}</span>}
                                    </>
                                ),
                            },
                            {
                                header: "Capacity",
                                width: "22%",
                                cell: (c) => (
                                    <TaskMeter running={c.tasks_running ?? 0} pending={c.tasks_pending ?? 0} target={c.services_count ?? 0} />
                                ),
                            },
                            { header: "Status", width: "13%", cell: (c) => <State health={clusterHealth(c)} /> },
                        ]}
                    />
                )}
            </div>
            {selected ? (
                <ClusterInspector profile={profile} active={active} cluster={selected} region={region} onOpen={onOpen} />
            ) : (
                <InspectorEmpty text={all ? "No cluster selected" : ""} />
            )}
        </>
    );
}

function ClusterInspector({
    profile,
    active,
    cluster,
    region,
    onOpen,
}: ViewProps & { cluster: EcsCluster; region: string; onOpen: (cluster: string) => void }) {
    const services = useResourceEnabled(active, ecsServicesR, profile, cluster.name);
    const running = cluster.tasks_running ?? 0;
    const pending = cluster.tasks_pending ?? 0;
    const list = services.data ?? [];
    const openService = (service: string) => setEcsLevel(profile, { kind: "service", cluster: cluster.name, service, tab: "logs" });
    return (
        <Inspector>
            <InspectorHead
                title={parseCluster(cluster.name).product}
                badge={<EnvChip name={cluster.name} />}
                sub={cluster.arn}
                actions={
                    <>
                        <button className="aws-btn primary" onClick={() => onOpen(cluster.name)}>
                            Open cluster
                        </button>
                        <ConsoleButton url={`${consoleBase(region)}/${cluster.name}/services?region=${region}`} />
                        <button className="aws-btn aws-icon-btn" onClick={() => copy(cluster.arn, "ARN")} title="Copy ARN" aria-label="Copy ARN">
                            <IconCopy size={14} />
                        </button>
                    </>
                }
            />
            <Stats
                items={[
                    { label: "Services", value: cluster.services_count ?? "—" },
                    { label: "Running", value: running },
                    { label: "Pending", value: pending, tone: pending ? "warn" : "zero" },
                ]}
            />
            <Section title="Tasks" meta={`${running}/${Math.max(cluster.services_count ?? 0, running)}`}>
                <TaskMeter running={running} pending={pending} target={cluster.services_count ?? 0} wrap />
            </Section>
            <Section title="Services" meta={services.data ? list.length : undefined}>
                {!services.data && <div className="aws-insp-note">{services.status === "error" ? services.error : "Loading services…"}</div>}
                {list.slice(0, 10).map((s) => (
                    <button key={s.arn} className="aws-insp-row" onClick={() => openService(s.name)} title="Tail this service's logs">
                        <State health={serviceHealth(s)} label="" />
                        <span className="aws-name aws-grow">{s.name}</span>
                        <span className="aws-num dim">
                            {s.running ?? 0}/{s.desired ?? 0}
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
    const handle = useResourceEnabled(active, ecsServicesR, profile, cluster);
    const [query, setQuery] = useState("");
    const picked = useAws((s) => s.selection[profile]?.service);
    const all = handle.data;
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
                            <EnvChip name={cluster} />
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
                                        <span className="aws-num">{s.running ?? 0}</span>
                                        <span className="aws-num dim">/{s.desired ?? 0}</span>
                                    </>
                                ),
                            },
                            { header: "Pending", width: "9%", align: "right", cell: (s) => <Num n={s.pending ?? 0} /> },
                            {
                                header: "Tasks",
                                width: "19%",
                                cell: (s) => <TaskMeter running={s.running ?? 0} pending={s.pending ?? 0} target={s.desired ?? 0} />,
                            },
                            {
                                header: "Deployed",
                                width: "13%",
                                cell: (s) => <span className="aws-num dim">{relative(s.primary_updated_at ?? s.primary_created_at)}</span>,
                            },
                            {
                                header: "Status",
                                width: "14%",
                                cell: (s) => <State health={serviceHealth(s)} label={SERVICE_STATE[serviceHealth(s)]} />,
                            },
                        ]}
                    />
                )}
            </div>
            {selected ? (
                <ServiceInspector
                    profile={profile}
                    active={active}
                    cluster={cluster}
                    service={selected}
                    region={region}
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
    profile,
    active,
    cluster,
    service,
    region,
    onOpen,
    onOpenTask,
}: ViewProps & {
    cluster: string;
    service: EcsService;
    region: string;
    onOpen: (tab: "logs" | "tasks") => void;
    onOpenTask: (filter: { taskId: string; stream: string }) => void;
}) {
    const logs = useResourceEnabled(active, ecsServiceLogConfigR, profile, cluster, service.name);
    const tasks = useResourceEnabled(active, ecsTasksR, profile, cluster, service.name);
    const pending = service.pending ?? 0;
    const openTask = (t: EcsTask) => {
        void awsApi
            .ecsTaskLogConfig(profile, cluster, t.arn)
            .then((cfg) => onOpenTask({ taskId: t.task_id, stream: cfg.log_stream }))
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
                        <ConsoleButton url={`${consoleBase(region)}/${cluster}/services/${service.name}/health?region=${region}`} />
                    </>
                }
            />
            <Stats
                items={[
                    { label: "Desired", value: service.desired ?? 0 },
                    { label: "Running", value: service.running ?? 0 },
                    { label: "Pending", value: pending, tone: pending ? "warn" : "zero" },
                ]}
            />
            <Facts
                items={[
                    { label: "Status", value: SERVICE_STATE[serviceHealth(service)] },
                    { label: "Deployed", value: relative(service.primary_updated_at ?? service.primary_created_at) },
                    { label: "Log group", value: logs.data?.log_group ?? (logs.status === "loading" ? "…" : null), copy: true },
                ]}
            />
            <Section title="Tasks" meta={tasks.data?.length}>
                {!tasks.data && <div className="aws-insp-note">{tasks.status === "error" ? tasks.error : "Loading tasks…"}</div>}
                {tasks.data?.length === 0 && <div className="aws-insp-note">No tasks running.</div>}
                {tasks.data?.slice(0, 8).map((t) => (
                    <button key={t.arn} className="aws-insp-row" onClick={() => openTask(t)} title="Tail this task's logs">
                        <State health={t.status === "RUNNING" ? "ok" : "warn"} label="" />
                        <span className="aws-mono aws-grow">{t.task_id.slice(0, 12)}</span>
                        <span className="aws-num dim">
                            {t.availability_zone ? `${t.availability_zone.slice(-2)} · ` : ""}
                            {relative(t.started_at)}
                        </span>
                        <IconChevron size={12} />
                    </button>
                ))}
            </Section>
        </Inspector>
    );
}

function ServicePage({
    profile,
    active,
    level,
    setLevel,
}: ViewProps & { level: Extract<EcsLevel, { kind: "service" }>; setLevel: (l: EcsLevel) => void }) {
    const region = useRegion(profile);
    const { cluster, service, tab, taskFilter } = level;
    const services = useResourceEnabled(active, ecsServicesR, profile, cluster);
    const tasks = useResourceEnabled(active, ecsTasksR, profile, cluster, service);
    const cfg = useResourceEnabled(active, ecsServiceLogConfigR, profile, cluster, service);
    const info = services.data?.find((s) => s.name === service);
    const toClusters = () => setLevel({ kind: "clusters" });

    const lead = (
        <>
            <Seg
                options={[
                    { value: "logs" as const, label: "Logs" },
                    { value: "tasks" as const, label: "Tasks", count: tasks.data?.length },
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
                            <State health={serviceHealth(info)} label={SERVICE_STATE[serviceHealth(info)]} />
                            <span className="aws-num dim">
                                {info.running ?? 0}/{info.desired ?? 0} tasks · deployed{" "}
                                {relative(info.primary_updated_at ?? info.primary_created_at)}
                            </span>
                        </span>
                    )
                }
                tools={<ConsoleButton url={`${consoleBase(region)}/${cluster}/services/${service}/health?region=${region}`} />}
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
    handle: ResourceHandle<EcsTask[]>;
    onPick: (taskId: string, stream: string) => void;
}) {
    const open = (t: EcsTask) => {
        void awsApi
            .ecsTaskLogConfig(profile, cluster, t.arn)
            .then((cfg) => onPick(t.task_id, cfg.log_stream))
            .catch(reportError("task log config"));
    };
    return (
        <>
            <ListState handle={handle} loading="Loading tasks…" empty="No tasks running." count={handle.data?.length} />
            {!!handle.data?.length && (
                <Table
                    label="Tasks"
                    rows={handle.data}
                    rowKey={(t) => t.arn}
                    onOpen={open}
                    columns={[
                        {
                            header: "Task",
                            width: "22%",
                            cell: (t) => (
                                <span className="aws-mono">
                                    <span className="aws-name">{t.task_id.slice(0, 12)}</span>
                                    <span className="aws-name-dim">{t.task_id.slice(12, 20)}…</span>
                                </span>
                            ),
                        },
                        {
                            header: "Status",
                            width: "14%",
                            cell: (t) => (
                                <State health={t.status === "RUNNING" ? "ok" : t.status === "STOPPED" ? "off" : "warn"} label={titleCase(t.status)} />
                            ),
                        },
                        {
                            header: "Health",
                            width: "11%",
                            cell: (t) => (
                                <span
                                    className={`aws-health ${t.health_status === "HEALTHY" ? "ok" : t.health_status === "UNHEALTHY" ? "fail" : ""}`}>
                                    {titleCase(t.health_status)}
                                </span>
                            ),
                        },
                        { header: "Private IP", width: "13%", cell: (t) => <Cell>{t.private_ip ?? "—"}</Cell> },
                        { header: "Zone", width: "12%", cell: (t) => <Cell>{t.availability_zone ?? "—"}</Cell> },
                        { header: "Size", width: "14%", cell: (t) => <Cell dim>{taskSize(t)}</Cell> },
                        { header: "Started", width: "10%", cell: (t) => <Cell dim>{relative(t.started_at)}</Cell> },
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

function titleCase(value: string | null): string {
    if (!value) return "—";
    return value[0] + value.slice(1).toLowerCase();
}

function taskSize(t: EcsTask): string {
    const cpu = Number(t.cpu);
    const mem = Number(t.memory);
    if (!Number.isFinite(cpu) || !Number.isFinite(mem) || !cpu || !mem) return "—";
    return `${cpu / 1024} vCPU · ${mem >= 1024 ? `${mem / 1024} GB` : `${mem} MB`}`;
}
