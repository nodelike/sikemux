import type { ReactNode } from "react";
import { reportError } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { IconChevron } from "../../../plugin-api/ui";
import { awsApi, type EcsCluster, type EcsService, type EcsTask } from "../api";
import { ecsClustersR, ecsServiceLogConfigR, ecsServicesR, ecsTasksR } from "../resources";
import { setEcsLevel, useAws, type EcsLevel } from "../state";
import { AwsLogTailView } from "./AwsLogTailView";
import { AwsRefresh } from "./AwsRefresh";

interface ViewProps {
    profile: string;
    active: boolean;
}

function relative(iso: string | null): string {
    if (!iso) return "—";
    const t = Date.parse(iso);
    if (Number.isNaN(t)) return "—";
    const d = Math.max(0, (Date.now() - t) / 1000);
    if (d < 60) return `${Math.floor(d)}s ago`;
    if (d < 3600) return `${Math.floor(d / 60)}m ago`;
    if (d < 86400) return `${Math.floor(d / 3600)}h ago`;
    if (d < 86400 * 30) return `${Math.floor(d / 86400)}d ago`;
    return `${Math.floor(d / (86400 * 30))}mo ago`;
}

function statusOf(s: EcsService): "ok" | "warn" | "fail" | "off" {
    const desired = s.desired ?? 0;
    const running = s.running ?? 0;
    const pending = s.pending ?? 0;
    if (pending > 0) return "warn";
    if (desired === 0 && running === 0) return "off";
    if (running === desired && desired > 0) return "ok";
    return "fail";
}

const DEFAULT_LEVEL: EcsLevel = { kind: "clusters" };

type EcsColumn<T> = {
    header: string;
    className?: string;
    render: (row: T) => ReactNode;
};

function EcsResourceTable<T>({
    refresh,
    data,
    error,
    status,
    loading,
    empty,
    columns,
    rowKey,
    rowTitle,
    onPick,
}: {
    refresh?: ReactNode;
    data: T[] | undefined;
    error?: string;
    status: string;
    loading: string;
    empty: string;
    columns: EcsColumn<T>[];
    rowKey: (row: T) => string;
    rowTitle?: (row: T) => string | undefined;
    onPick: (row: T) => void;
}) {
    const message =
        status === "error" && error ? (
            <div className="aws-err">{error}</div>
        ) : !data ? (
            <div className="aws-loading">{loading}</div>
        ) : data.length === 0 ? (
            <div className="aws-empty">{empty}</div>
        ) : null;
    if (message)
        return (
            <>
                {refresh}
                {message}
            </>
        );
    const rows = data ?? [];
    return (
        <>
            {refresh}
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
                    {rows.map((row) => (
                        <tr
                            key={rowKey(row)}
                            onClick={() => onPick(row)}
                            onKeyDown={(event) => {
                                if (event.key === "Enter" || event.key === " ") {
                                    event.preventDefault();
                                    onPick(row);
                                }
                            }}
                            tabIndex={0}
                            role="button"
                            aria-label={`Open ${rowKey(row)}`}
                            className="aws-row"
                            title={rowTitle?.(row)}>
                            {columns.map((c) => (
                                <td key={c.header} className={c.className}>
                                    {c.render(row)}
                                </td>
                            ))}
                        </tr>
                    ))}
                </tbody>
            </table>
        </>
    );
}

const CLUSTER_COLUMNS: EcsColumn<EcsCluster>[] = [
    { header: "Cluster", className: "aws-col-name", render: (c) => c.name },
    { header: "Services", render: (c) => c.services_count ?? "—" },
    { header: "Running", render: (c) => c.tasks_running ?? "—" },
    { header: "Pending", render: (c) => c.tasks_pending ?? "—" },
    {
        header: "Status",
        render: (c) => <span className={`aws-status aws-status-${c.status === "ACTIVE" ? "ok" : "off"}`}>{c.status ?? "—"}</span>,
    },
];

const SERVICE_COLUMNS: EcsColumn<EcsService>[] = [
    { header: "Service", className: "aws-col-name", render: (s) => s.name },
    { header: "Running", render: (s) => s.running ?? 0 },
    { header: "Desired", render: (s) => s.desired ?? 0 },
    { header: "Pending", render: (s) => s.pending ?? 0 },
    { header: "Updated", render: (s) => relative(s.primary_updated_at ?? s.primary_created_at) },
    { header: "Status", render: (s) => <span className={`aws-status aws-status-${statusOf(s)}`}>{statusOf(s)}</span> },
];

const TASK_COLUMNS: EcsColumn<EcsTask>[] = [
    { header: "Task", className: "aws-col-name", render: (t) => t.task_id.slice(0, 12) },
    { header: "Status", render: (t) => t.status ?? "—" },
    { header: "Health", render: (t) => t.health_status ?? "—" },
    { header: "CPU", render: (t) => t.cpu ?? "—" },
    { header: "Mem", render: (t) => t.memory ?? "—" },
    { header: "Started", render: (t) => relative(t.started_at) },
];

export function AwsEcsView({ profile, active }: ViewProps) {
    const level = useAws((s) => s.ecsViews[profile] ?? DEFAULT_LEVEL);
    const setLevel = (l: EcsLevel) => setEcsLevel(profile, l);

    return (
        <div className="aws-view">
            <Breadcrumb level={level} onJump={setLevel} />
            {level.kind === "clusters" && (
                <ClustersList profile={profile} active={active} onPick={(c) => setLevel({ kind: "services", cluster: c })} />
            )}
            {level.kind === "services" && (
                <ServicesList
                    profile={profile}
                    active={active}
                    cluster={level.cluster}
                    onPick={(s) =>
                        setLevel({
                            kind: "service",
                            cluster: level.cluster,
                            service: s,
                            tab: "logs",
                        })
                    }
                />
            )}
            {level.kind === "service" && (
                <ServiceView
                    profile={profile}
                    active={active}
                    cluster={level.cluster}
                    service={level.service}
                    tab={level.tab}
                    taskFilter={level.taskFilter}
                    onTab={(tab) => setLevel({ ...level, tab, taskFilter: level.taskFilter })}
                    onPickTask={(taskId, stream) =>
                        setLevel({
                            kind: "service",
                            cluster: level.cluster,
                            service: level.service,
                            tab: "logs",
                            taskFilter: { taskId, stream },
                        })
                    }
                    onClearFilter={() =>
                        setLevel({
                            kind: "service",
                            cluster: level.cluster,
                            service: level.service,
                            tab: level.tab,
                        })
                    }
                />
            )}
        </div>
    );
}

function Breadcrumb({ level, onJump }: { level: EcsLevel; onJump: (l: EcsLevel) => void }) {
    const parts: { label: string; jump: EcsLevel }[] = [{ label: "Clusters", jump: { kind: "clusters" } }];
    if (level.kind !== "clusters") {
        parts.push({
            label: level.cluster,
            jump: { kind: "services", cluster: level.cluster },
        });
    }
    if (level.kind === "service") {
        parts.push({
            label: level.service,
            jump: {
                kind: "service",
                cluster: level.cluster,
                service: level.service,
                tab: "logs",
            },
        });
        if (level.taskFilter) {
            parts.push({
                label: level.taskFilter.taskId.slice(0, 12),
                jump: level,
            });
        }
    }
    return (
        <div className="aws-breadcrumb">
            {parts.map((p, i) => (
                <span className="aws-breadcrumb-seg" key={i}>
                    {i > 0 && <IconChevron size={11} className="aws-breadcrumb-sep" />}
                    {i < parts.length - 1 ? (
                        <button className="aws-breadcrumb-link" onClick={() => onJump(p.jump)}>
                            {p.label}
                        </button>
                    ) : (
                        <span className="aws-breadcrumb-here">{p.label}</span>
                    )}
                </span>
            ))}
        </div>
    );
}

function ClustersList({ profile, active, onPick }: { profile: string; active: boolean; onPick: (cluster: string) => void }) {
    const handle = useResourceEnabled(active, ecsClustersR, profile);
    const { data, error, status } = handle;
    return (
        <EcsResourceTable
            refresh={<AwsRefresh handle={handle} />}
            data={data}
            error={error}
            status={status}
            loading="loading clusters…"
            empty="no clusters"
            columns={CLUSTER_COLUMNS}
            rowKey={(c) => c.arn}
            onPick={(c) => onPick(c.name)}
        />
    );
}

function ServicesList({
    profile,
    active,
    cluster,
    onPick,
}: {
    profile: string;
    active: boolean;
    cluster: string;
    onPick: (service: string) => void;
}) {
    const handle = useResourceEnabled(active, ecsServicesR, profile, cluster);
    const { data, error, status } = handle;
    return (
        <EcsResourceTable
            refresh={<AwsRefresh handle={handle} />}
            data={data}
            error={error}
            status={status}
            loading="loading services…"
            empty="no services"
            columns={SERVICE_COLUMNS}
            rowKey={(s) => s.arn}
            rowTitle={() => "Click → service logs (live tail)"}
            onPick={(s) => onPick(s.name)}
        />
    );
}

function ServiceView({
    profile,
    active,
    cluster,
    service,
    tab,
    taskFilter,
    onTab,
    onPickTask,
    onClearFilter,
}: {
    profile: string;
    active: boolean;
    cluster: string;
    service: string;
    tab: "logs" | "tasks";
    taskFilter?: { taskId: string; stream: string };
    onTab: (t: "logs" | "tasks") => void;
    onPickTask: (taskId: string, stream: string) => void;
    onClearFilter: () => void;
}) {
    const cfg = useResourceEnabled(active, ecsServiceLogConfigR, profile, cluster, service);

    return (
        <div className="aws-view">
            <div className="aws-subtabs">
                <button className={`aws-subtab${tab === "logs" ? " active" : ""}`} onClick={() => onTab("logs")}>
                    Logs
                </button>
                <button className={`aws-subtab${tab === "tasks" ? " active" : ""}`} onClick={() => onTab("tasks")}>
                    Tasks
                </button>
                {taskFilter && tab === "logs" && (
                    <div className="aws-subtab-filter">
                        filtered to <code>{taskFilter.taskId.slice(0, 12)}</code>
                        <button className="aws-subtab-clear" onClick={onClearFilter}>
                            clear ✕
                        </button>
                    </div>
                )}
            </div>

            {tab === "logs" && (
                <>
                    {cfg.error ? (
                        <div className="aws-err">{cfg.error}</div>
                    ) : !cfg.data ? (
                        <div className="aws-loading">resolving service log group…</div>
                    ) : (
                        <AwsLogTailView
                            key={`${cfg.data.log_group}|${taskFilter?.stream ?? ""}`}
                            profile={profile}
                            logGroup={cfg.data.log_group}
                            logStream={taskFilter?.stream ?? null}
                            active={active}
                        />
                    )}
                </>
            )}

            {tab === "tasks" && (
                <TasksList
                    profile={profile}
                    active={active}
                    cluster={cluster}
                    service={service}
                    onPick={(t) => {
                        void awsApi
                            .ecsTaskLogConfig(profile, cluster, t.arn)
                            .then((cfg) => onPickTask(t.task_id, cfg.log_stream))
                            .catch((e) => reportError("task log config")(e));
                    }}
                />
            )}
        </div>
    );
}

function TasksList({
    profile,
    active,
    cluster,
    service,
    onPick,
}: {
    profile: string;
    active: boolean;
    cluster: string;
    service: string;
    onPick: (t: EcsTask) => void;
}) {
    const { data, error, status } = useResourceEnabled(active, ecsTasksR, profile, cluster, service);
    return (
        <EcsResourceTable
            data={data}
            error={error}
            status={status}
            loading="loading tasks…"
            empty="no tasks"
            columns={TASK_COLUMNS}
            rowKey={(t) => t.arn}
            rowTitle={() => "Click → filter logs to this task"}
            onPick={onPick}
        />
    );
}
