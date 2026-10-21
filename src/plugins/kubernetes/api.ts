import { createPluginBackend, type PluginStream, type PluginStreamHandlers } from "../../plugin-api/backend";
import { KUBERNETES_PLUGIN } from "./kinds";

export { isPluginFailure } from "../../plugin-api/backend";

const backend = createPluginBackend(KUBERNETES_PLUGIN.id);

/** Which cluster and namespace a call is for; none leaves kubectl's current ones. */
export interface Scope {
    context?: string;
    namespace?: string;
}

export interface KubeContext {
    name: string;
    cluster: string;
    server: string | null;
    namespace: string | null;
}

export interface KubeStatus {
    /** kubectl is installed and the kubeconfig names at least one cluster. */
    ready: boolean;
    message: string | null;
    /** cli-missing or unconfigured when not ready. */
    category: string | null;
    current: string | null;
    contexts: KubeContext[];
}

export interface Namespace {
    name: string;
    phase: string;
}

export interface Owner {
    kind: string;
    name: string;
}

export interface Pod {
    name: string;
    namespace: string;
    /** As `kubectl get pods` shows STATUS: Running, CrashLoopBackOff, Completed, Init:0/1. */
    status: string;
    phase: string;
    ready: number;
    containers: number;
    restarts: number;
    created: string | null;
    node: string | null;
    ip: string | null;
    owner: Owner | null;
}

export interface Ended {
    reason: string | null;
    exitCode: number | null;
    finished: string | null;
}

export interface Container {
    name: string;
    image: string;
    init: boolean;
    ready: boolean;
    restarts: number;
    state: "running" | "waiting" | "terminated" | string;
    reason: string | null;
    message: string | null;
    exitCode: number | null;
    since: string | null;
    last: Ended | null;
    ports: string[];
    /** Names only, with where each is set from. Values never reach the pane. */
    env: { name: string; from: string }[];
    envFrom: string[];
    requests: [string, string][];
    limits: [string, string][];
    probes: string[];
}

export interface KubeEvent {
    kind: "Normal" | "Warning" | string;
    reason: string;
    message: string;
    count: number;
    last: string | null;
    object: string;
    namespace: string | null;
}

export interface PodDetail extends Pod {
    labels: [string, string][];
    conditions: { kind: string; ok: boolean; reason: string | null; message: string | null }[];
    /** Each container's state, init containers first. */
    containerStates: Container[];
    volumes: { name: string; kind: string; source: string | null }[];
    events: KubeEvent[];
}

export interface LogLine {
    at: string | null;
    text: string;
}

export interface Workload {
    kind: "Deployment" | "StatefulSet" | "DaemonSet" | "Job" | "CronJob" | string;
    name: string;
    namespace: string;
    ready: number | null;
    desired: number | null;
    health: "ok" | "progressing" | "warning" | "failed";
    note: string | null;
    images: string[];
    created: string | null;
    selector: string | null;
    schedule: string | null;
    suspended: boolean | null;
    lastRun: string | null;
}

export interface Service {
    name: string;
    namespace: string;
    kind: string;
    clusterIp: string | null;
    external: string[];
    ports: string[];
    selector: string | null;
}

export interface Ingress {
    name: string;
    namespace: string;
    class: string | null;
    rules: { host: string; path: string; service: string; port: string }[];
    tls: string[];
    address: string[];
}

export interface Store {
    name: string;
    namespace: string;
    kind: string | null;
    keys: { name: string; size: number }[];
}

export type KubeAction = "restart" | "scale" | "deletePod" | "runNow" | "suspend" | "resume" | "undo" | "pauseRollout" | "resumeRollout";

export interface KubeNode {
    name: string;
    /** Such as control-plane or worker. */
    roles: string[];
    /** Ready, NotReady or Unknown. */
    status: string;
    /** No new pods are placed on it. */
    cordoned: boolean;
    /** MemoryPressure, DiskPressure, PIDPressure or NetworkUnavailable, whichever are on. */
    pressure: string[];
    reason: string | null;
    version: string;
    os: string;
    arch: string;
    runtime: string;
    internalIp: string | null;
    cpuMillis: number;
    memoryBytes: number;
    maxPods: number;
    /** Only when the cluster runs metrics-server. */
    cpuUsedMillis: number | null;
    memoryUsedBytes: number | null;
    taints: string[];
    created: string | null;
}

export interface Revision {
    revision: number;
    name: string;
    current: boolean;
    changeCause: string | null;
    images: string[];
    /** Made by a restart rather than a change. */
    restart: boolean;
    created: string | null;
    pods: number | null;
}

export interface RolloutHistory {
    kind: string;
    name: string;
    paused: boolean;
    /** Made with kubectl apply, so the next apply of the old file reverts an undo made here. */
    applied: boolean;
    current: number | null;
    /** Newest first. */
    revisions: Revision[];
}

export interface HelmRelease {
    name: string;
    namespace: string;
    revision: number;
    updated: string | null;
    /** deployed, failed, pending-install, pending-upgrade, pending-rollback, superseded or uninstalling. */
    status: string;
    chart: string;
    chartVersion: string | null;
    appVersion: string | null;
}

export interface HelmStep {
    revision: number;
    updated: string | null;
    status: string;
    chart: string;
    chartVersion: string | null;
    appVersion: string | null;
    description: string;
}

export interface HelmDetail {
    /** Newest first. */
    history: HelmStep[];
    /** What the release made, by kind and name only. */
    made: { kind: string; name: string; namespace: string | null }[];
}

export const kubeApi = {
    status: () => backend.call<KubeStatus>("status"),
    namespaces: (scope: Scope) => backend.call<Namespace[]>("namespaces", scope),
    pods: (scope: Scope, allNamespaces: boolean, selector: string | null) =>
        backend.call<Pod[]>("pods", { ...scope, allNamespaces, selector: selector ?? undefined }),
    pod: (scope: Scope, name: string) => backend.call<PodDetail | null>("pod", { ...scope, name }),
    logs: (scope: Scope, pod: string, container: string | null, previous: boolean) =>
        backend.call<{ lines: LogLine[]; filtered: number }>("logs", { ...scope, pod, container, previous, tail: 1000 }),
    followLogs: (
        scope: Scope,
        pod: string,
        container: string | null,
        handlers: PluginStreamHandlers<{ lines: LogLine[]; ended: boolean }>,
    ): PluginStream => backend.stream("followLogs", { ...scope, pod, container, tail: 1000 }, handlers),
    workloads: (scope: Scope, allNamespaces: boolean) => backend.call<Workload[]>("workloads", { ...scope, allNamespaces }),
    events: (scope: Scope, allNamespaces: boolean) => backend.call<KubeEvent[]>("events", { ...scope, allNamespaces, limit: 300 }),
    network: (scope: Scope, allNamespaces: boolean) =>
        backend.call<{ services: Service[]; ingresses: Ingress[] }>("network", { ...scope, allNamespaces }),
    config: (scope: Scope, allNamespaces: boolean) => backend.call<{ configMaps: Store[]; secrets: Store[] }>("config", { ...scope, allNamespaces }),
    act: (scope: Scope, action: KubeAction, name: string, kind?: string, replicas?: number, revision?: number) =>
        backend.call<{ message: string }>("act", { ...scope, action, name, kind, replicas, revision }),
    nodes: (context: string) => backend.call<KubeNode[]>("nodes", { context }),
    cordon: (context: string, name: string, cordon: boolean) => backend.call<{ message: string }>("cordon", { context, name, cordon }),
    history: (scope: Scope, kind: string, name: string) => backend.call<RolloutHistory>("history", { ...scope, kind, name }),
    helmReleases: (scope: Scope, allNamespaces: boolean) => backend.call<HelmRelease[]>("helmReleases", { ...scope, allNamespaces }),
    helmRelease: (scope: Scope, name: string) => backend.call<HelmDetail>("helmRelease", { ...scope, name }),
    helmRollback: (scope: Scope, name: string, revision: number) => backend.call<{ message: string }>("helmRollback", { ...scope, name, revision }),
    yaml: (scope: Scope, kind: string, name: string) => backend.call<string>("yaml", { ...scope, kind, name }),
};
