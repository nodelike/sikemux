import { errorText } from "../../../plugin-api/format";
import { useEffect, useState } from "react";
import { notify, promptDialog } from "../../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, SkeletonRows } from "../../../plugin-api/ui";
import { gcpApi, type GkeCluster, type Pod, type PodLogLine, type Target, type Workload } from "../api";
import { clustersR, kubeServicesR, nodesR, podEventsR, podsR, workloadsR } from "../resources";
import { splitOpen, updateGcpView, useGcpView } from "../state";
import type { ProductProps } from "./GcpPane";
import { LogStream } from "./LogStream";
import { SendToAgent } from "./SendToAgent";
import { podForAgent } from "../forAgent";
import { Filter } from "./RunPage";
import { ago, Button, consoleUrl, Facts, keyOf, Loaded, PageHeader, Status, Table, Tabs, useAction, visit } from "./parts";
import { Problem } from "./Problem";

export function GkePage({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    const clusters = useResourceEnabled(active, clustersR, ...keyOf(target));
    const opened = splitOpen(view.open);
    const cluster = opened && clusters.data?.find((each) => each.location === opened.location && each.name === opened.name);
    if (opened && cluster)
        return (
            <ClusterPage
                paneId={paneId}
                active={active}
                target={target}
                cluster={cluster}
                onBack={() => updateGcpView(paneId, { open: null, tab: null })}
            />
        );
    const words = view.filter.trim().toLowerCase();
    return (
        <div className="gcp-page">
            <PageHeader title="Kubernetes clusters">
                <Button onClick={() => visit(consoleUrl("kubernetes/list/overview", target.project))}>Create cluster in Cloud console</Button>
            </PageHeader>
            <Filter paneId={paneId} placeholder="Filter clusters" />
            <Loaded handle={clusters} label="clusters" empty={(rows) => (rows.length === 0 ? "No GKE clusters in this project." : null)}>
                {(rows) => (
                    <Table
                        label="Clusters"
                        rows={rows.filter((each) => !words || each.name.toLowerCase().includes(words))}
                        rowKey={(each) => `${each.location}/${each.name}`}
                        onOpen={(each: GkeCluster) => updateGcpView(paneId, { open: `${each.location}/${each.name}`, tab: null })}
                        columns={[
                            { label: "Status", width: "70px", cell: (each) => <Status state={each.status} /> },
                            { label: "Name", width: "minmax(140px, 1.2fr)", cell: (each) => <span className="gcp-name">{each.name}</span> },
                            { label: "Location", cell: (each) => each.location },
                            { label: "Mode", cell: (each) => (each.autopilot ? "Autopilot" : "Standard") },
                            { label: "Nodes", cell: (each) => each.nodes ?? "—" },
                            { label: "Version", cell: (each) => each.version ?? "—" },
                            { label: "Channel", cell: (each) => each.channel?.toLowerCase() ?? "—" },
                        ]}
                    />
                )}
            </Loaded>
        </div>
    );
}

const TABS = [
    { value: "nodes", label: "Nodes" },
    { value: "workloads", label: "Workloads" },
    { value: "pods", label: "Pods" },
    { value: "services", label: "Services" },
    { value: "logs", label: "Logs" },
] as const;

function ClusterPage({
    paneId,
    active,
    target,
    cluster,
    onBack,
}: {
    paneId: string;
    active: boolean;
    target: Target;
    cluster: GkeCluster;
    onBack: () => void;
}) {
    const view = useGcpView(paneId);
    const tab = TABS.find((each) => each.value === view.tab)?.value ?? (cluster.autopilot ? "workloads" : "nodes");
    const [pod, setPod] = useState<Pod | null>(null);
    return (
        <div className="gcp-page">
            <PageHeader title={cluster.name} back={{ label: "clusters", go: onBack }}>
                <Button onClick={() => visit(consoleUrl(`kubernetes/clusters/details/${cluster.location}/${cluster.name}/details`, target.project))}>
                    Open in Cloud console
                </Button>
            </PageHeader>
            <div className="gcp-summary">
                <Status state={cluster.status} text />
                <span className="gcp-muted">{cluster.location}</span>
                <span className="gcp-muted">{cluster.autopilot ? "Autopilot" : "Standard"}</span>
                <span className="gcp-muted">Kubernetes {cluster.version}</span>
                {cluster.private && <span className="gcp-chip">Private endpoint</span>}
            </div>
            <Tabs
                label={cluster.name}
                tabs={TABS}
                value={tab}
                onChange={(value) => {
                    setPod(null);
                    updateGcpView(paneId, { tab: value });
                }}
            />
            {tab === "nodes" && (
                <>
                    <NodePools target={target} cluster={cluster} />
                    <Nodes active={active} target={target} cluster={cluster} />
                </>
            )}
            {tab === "workloads" && (
                <Workloads
                    active={active}
                    target={target}
                    cluster={cluster}
                    onPods={(workload) => {
                        updateGcpView(paneId, { tab: "pods", filter: workload.selector });
                    }}
                />
            )}
            {tab === "pods" &&
                (pod ? (
                    <PodLogs target={target} cluster={cluster} pod={pod} onBack={() => setPod(null)} />
                ) : (
                    <Pods paneId={paneId} active={active} target={target} cluster={cluster} onOpen={setPod} />
                ))}
            {tab === "services" && <Services active={active} target={target} cluster={cluster} />}
            {tab === "logs" && <LogStream active={active} target={target} query={{ resource: "gkeCluster", name: cluster.name }} />}
        </div>
    );
}

async function askCount(title: string, body: string, initial: number): Promise<number | null> {
    const answer = await promptDialog({ title, body, label: "Count", initial: String(initial), confirmLabel: "Set" });
    if (answer === null) return null;
    const count = Number(answer.trim());
    if (!Number.isInteger(count) || count < 0 || count > 1000) {
        notify("error", `${answer} is not a count from 0 to 1000`);
        return null;
    }
    return count;
}

async function change(run: () => Promise<unknown>, done: string, failed: string) {
    try {
        await run();
        notify("success", done);
        invalidate((kind) => kind.startsWith("gcp."));
    } catch (error) {
        notify("error", `${failed}: ${errorText(error)}`);
    }
}

function Nodes({ active, target, cluster }: { active: boolean; target: Target; cluster: GkeCluster }) {
    const nodes = useResourceEnabled(active, nodesR, ...keyOf(target), cluster.location, cluster.name);
    return (
        <>
            <h2 className="gcp-section-title">Nodes</h2>
            <Loaded handle={nodes} label="nodes" empty={(rows) => (rows.length === 0 ? "The cluster has no nodes right now." : null)}>
                {(rows) => (
                    <Table
                        label="Nodes"
                        rows={rows}
                        rowKey={(node) => node.name}
                        columns={[
                            {
                                label: "Status",
                                width: "70px",
                                cell: (node) => (
                                    <Status state={!node.ready ? "FAILED" : node.pressure.length > 0 || node.unschedulable ? "DEGRADED" : "READY"} />
                                ),
                            },
                            {
                                label: "Name",
                                width: "minmax(200px, 1.6fr)",
                                cell: (node) => <span title={node.pressure.join(", ") || undefined}>{node.name}</span>,
                            },
                            { label: "Pool", cell: (node) => node.pool ?? "—" },
                            { label: "Machine", cell: (node) => `${node.machineType ?? "—"}${node.spot ? " (Spot)" : ""}` },
                            { label: "CPU", cell: (node) => node.cpu ?? "—" },
                            { label: "Memory", cell: (node) => node.memory ?? "—" },
                            { label: "Zone", cell: (node) => node.zone ?? "—" },
                        ]}
                    />
                )}
            </Loaded>
        </>
    );
}

function NodePools({ target, cluster }: { target: Target; cluster: GkeCluster }) {
    if (cluster.autopilot) return <EmptyState message="Autopilot manages this cluster's node pools for you." />;
    const resize = async (pool: GkeCluster["nodePools"][number]) => {
        const nodes = await askCount(`Resize ${pool.name}`, "How many nodes the pool runs in each of its zones.", pool.nodes ?? 0);
        if (nodes !== null)
            await change(
                () => gcpApi.nodePoolResize(target, cluster.location, cluster.name, pool.name, nodes),
                `${pool.name} is resizing to ${nodes}`,
                "Resize",
            );
    };
    return (
        <Table
            label="Node pools"
            rows={cluster.nodePools}
            rowKey={(pool) => pool.name}
            columns={[
                { label: "Status", width: "70px", cell: (pool) => <Status state={pool.status} /> },
                { label: "Name", width: "minmax(140px, 1.2fr)", cell: (pool) => pool.name },
                { label: "Machine type", cell: (pool) => `${pool.machineType ?? "—"}${pool.spot ? " (Spot)" : ""}` },
                { label: "Nodes", cell: (pool) => (pool.autoscaling ? `${pool.minNodes}–${pool.maxNodes} (autoscaling)` : (pool.nodes ?? "—")) },
                { label: "Version", cell: (pool) => pool.version ?? "—" },
                { label: "", width: "100px", align: "end", cell: (pool) => <Button onClick={() => void resize(pool)}>Resize</Button> },
            ]}
        />
    );
}

function Workloads({
    active,
    target,
    cluster,
    onPods,
}: {
    active: boolean;
    target: Target;
    cluster: GkeCluster;
    onPods: (workload: Workload) => void;
}) {
    const workloads = useResourceEnabled(active, workloadsR, ...keyOf(target), cluster.location, cluster.name);
    const action = useAction();
    const restart = (workload: Workload) =>
        void action.run(
            {
                title: `Restart ${workload.name}?`,
                body: `Kubernetes replaces ${workload.name}'s pods one at a time, as kubectl rollout restart does.`,
                confirmLabel: "Restart",
            },
            `${workload.name} is restarting`,
            () => gcpApi.deploymentRestart(target, cluster.location, cluster.name, workload.namespace, workload.name),
        );
    const scale = async (workload: Workload) => {
        const replicas = await askCount(`Scale ${workload.name}`, "How many pods the Deployment runs.", workload.desired);
        if (replicas !== null)
            await change(
                () => gcpApi.deploymentScale(target, cluster.location, cluster.name, workload.namespace, workload.name, replicas),
                `${workload.name} is scaling to ${replicas}`,
                "Scale",
            );
    };
    return (
        <Loaded handle={workloads} label="workloads" empty={(rows) => (rows.length === 0 ? "No workloads outside the system namespaces." : null)}>
            {(rows) => (
                <Table
                    label="Workloads"
                    rows={rows}
                    rowKey={(workload) => `${workload.kind}/${workload.namespace}/${workload.name}`}
                    columns={[
                        {
                            label: "Status",
                            width: "70px",
                            cell: (workload) => (
                                <Status
                                    state={
                                        workload.ready >= workload.desired
                                            ? "READY"
                                            : workload.ready === 0 && workload.desired > 0
                                              ? "FAILED"
                                              : "DEGRADED"
                                    }
                                />
                            ),
                        },
                        {
                            label: "Name",
                            width: "minmax(140px, 1.2fr)",
                            cell: (workload) => (
                                <button type="button" className="gcp-link" onClick={() => onPods(workload)}>
                                    {workload.name}
                                </button>
                            ),
                        },
                        { label: "Type", cell: (workload) => workload.kind },
                        { label: "Pods", cell: (workload) => `${workload.ready}/${workload.desired}` },
                        { label: "Namespace", cell: (workload) => workload.namespace },
                        {
                            label: "",
                            width: "minmax(150px, auto)",
                            align: "end",
                            cell: (workload) =>
                                workload.kind === "Deployment" ? (
                                    <>
                                        <Button disabled={action.busy} onClick={() => restart(workload)}>
                                            Restart
                                        </Button>
                                        <Button onClick={() => void scale(workload)}>Scale</Button>
                                    </>
                                ) : null,
                        },
                        {
                            label: "Image",
                            width: "minmax(160px, 1.4fr)",
                            cell: (workload) => (
                                <span className="gcp-muted">{workload.containers.map((container) => container.image).join(", ")}</span>
                            ),
                        },
                    ]}
                />
            )}
        </Loaded>
    );
}

function Pods({
    paneId,
    active,
    target,
    cluster,
    onOpen,
}: {
    paneId: string;
    active: boolean;
    target: Target;
    cluster: GkeCluster;
    onOpen: (pod: Pod) => void;
}) {
    const view = useGcpView(paneId);
    const selector = /^[\w./-]+=[\w.-]+(,[\w./-]+=[\w.-]+)*$/.test(view.filter.trim()) ? view.filter.trim() : "";
    const pods = useResourceEnabled(active, podsR, ...keyOf(target), cluster.location, cluster.name, selector);
    const words = selector ? "" : view.filter.trim().toLowerCase();
    return (
        <>
            <Filter paneId={paneId} placeholder="Filter pods, or a label selector such as app=api" />
            <Loaded handle={pods} label="pods" empty={(rows) => (rows.length === 0 ? "No pods outside the system namespaces." : null)}>
                {(rows) => (
                    <Table
                        label="Pods"
                        rows={rows.filter((pod) => !words || pod.name.toLowerCase().includes(words))}
                        rowKey={(pod) => `${pod.namespace}/${pod.name}`}
                        onOpen={onOpen}
                        columns={[
                            {
                                label: "Status",
                                width: "70px",
                                cell: (pod) => (
                                    <Status
                                        state={
                                            pod.reason
                                                ? "FAILED"
                                                : pod.phase === "Running"
                                                  ? "RUNNING"
                                                  : pod.phase === "Succeeded"
                                                    ? "SUCCEEDED"
                                                    : "PENDING"
                                        }
                                    />
                                ),
                            },
                            { label: "Name", width: "minmax(180px, 1.6fr)", cell: (pod) => <span className="gcp-name">{pod.name}</span> },
                            { label: "Ready", cell: (pod) => pod.ready },
                            { label: "State", cell: (pod) => pod.reason ?? pod.phase },
                            { label: "Restarts", cell: (pod) => pod.restarts },
                            { label: "Namespace", cell: (pod) => pod.namespace },
                            { label: "Age", cell: (pod) => ago(pod.createdAt) },
                        ]}
                    />
                )}
            </Loaded>
        </>
    );
}

function PodLogs({ target, cluster, pod, onBack }: { target: Target; cluster: GkeCluster; pod: Pod; onBack: () => void }) {
    const [previous, setPrevious] = useState(false);
    const [lines, setLines] = useState<PodLogLine[] | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [attempt, setAttempt] = useState(0);
    useEffect(() => {
        let current = true;
        setLines(null);
        setError(null);
        gcpApi
            .podLogs(target, cluster.location, cluster.name, pod.namespace, pod.name, previous)
            .then((answer) => current && setLines(answer.lines))
            .catch((failure: unknown) => current && setError(errorText(failure)));
        return () => {
            current = false;
        };
    }, [target, cluster.location, cluster.name, pod.namespace, pod.name, previous, attempt]);
    return (
        <div className="gcp-section">
            <PageHeader title={<>Logs: {pod.name}</>} back={{ label: "pods", go: onBack }}>
                <Button onClick={() => setPrevious(!previous)}>{previous ? "Current run" : "Previous run"}</Button>
                <SendToAgent disabled={!lines} delivery={() => podForAgent(target.project, cluster, pod, lines ?? [], previous)} />
            </PageHeader>
            <Facts
                items={[
                    ["Node", pod.node],
                    ["Pod IP", pod.ip],
                    ["Containers", pod.containers.map((container) => container.name).join(", ")],
                    ["Restarts", pod.restarts],
                ]}
            />
            <PodEvents target={target} cluster={cluster} pod={pod} />
            {error ? (
                <Problem error={error} what="pod logs" onRetry={() => setAttempt((n) => n + 1)} />
            ) : !lines ? (
                <SkeletonRows rows={8} label="Loading pod logs" />
            ) : lines.length === 0 ? (
                <EmptyState message="The pod printed nothing." />
            ) : (
                <pre className="gcp-terminal">{lines.map((line) => `${line.at ? `${line.at.slice(11, 19)}  ` : ""}${line.text}`).join("\n")}</pre>
            )}
        </div>
    );
}

function PodEvents({ target, cluster, pod }: { target: Target; cluster: GkeCluster; pod: Pod }) {
    const events = useResourceEnabled(true, podEventsR, ...keyOf(target), cluster.location, cluster.name, pod.namespace, pod.name);
    if (!events.data || events.data.length === 0) return null;
    return (
        <>
            <h2 className="gcp-section-title">Events</h2>
            <Table
                label="Events"
                rows={events.data}
                rowKey={(event) => `${event.reason}-${event.lastAt ?? ""}-${event.message}`}
                columns={[
                    {
                        label: "Type",
                        width: "80px",
                        cell: (event) => <span className={event.type === "Warning" ? "gcp-error-text" : undefined}>{event.type}</span>,
                    },
                    { label: "Reason", width: "minmax(110px, auto)", cell: (event) => event.reason },
                    { label: "Message", width: "minmax(220px, 3fr)", cell: (event) => <span title={event.message}>{event.message}</span> },
                    { label: "Count", width: "60px", cell: (event) => event.count },
                    { label: "Last seen", width: "90px", cell: (event) => ago(event.lastAt) },
                ]}
            />
        </>
    );
}

function Services({ active, target, cluster }: { active: boolean; target: Target; cluster: GkeCluster }) {
    const services = useResourceEnabled(active, kubeServicesR, ...keyOf(target), cluster.location, cluster.name);
    return (
        <Loaded handle={services} label="services" empty={(rows) => (rows.length === 0 ? "No services outside the system namespaces." : null)}>
            {(rows) => (
                <Table
                    label="Services"
                    rows={rows}
                    rowKey={(service) => `${service.namespace}/${service.name}`}
                    columns={[
                        { label: "Name", width: "minmax(140px, 1.2fr)", cell: (service) => service.name },
                        { label: "Type", cell: (service) => service.type },
                        {
                            label: "Endpoints",
                            width: "minmax(140px, 1.2fr)",
                            cell: (service) => service.externalIps.join(", ") || service.clusterIp || "—",
                        },
                        { label: "Ports", cell: (service) => service.ports.join(", ") },
                        { label: "Namespace", cell: (service) => service.namespace },
                    ]}
                />
            )}
        </Loaded>
    );
}
