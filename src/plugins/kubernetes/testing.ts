import type { HelmDetail, HelmRelease, KubeNode, KubeStatus, Pod, PodDetail, RolloutHistory, Workload } from "./api";

export const ready: KubeStatus = {
    ready: true,
    message: null,
    category: null,
    current: "kind-dev",
    contexts: [
        { name: "gke_acme_prod", cluster: "gke_acme_prod", server: "https://34.1.2.3", namespace: "shop" },
        { name: "kind-dev", cluster: "kind-dev", server: "https://127.0.0.1:6443", namespace: null },
    ],
};

export const noKubectl: KubeStatus = {
    ready: false,
    message: "kubernetes: kubectl isn't installed. Install it (brew install kubernetes-cli), then try again.",
    category: "cli-missing",
    current: null,
    contexts: [],
};

export function workload(name: string, patch: Partial<Workload> = {}): Workload {
    return {
        kind: "Deployment",
        name,
        namespace: "shop",
        ready: 2,
        desired: 2,
        health: "ok",
        note: null,
        images: ["nginx:alpine"],
        created: "2026-10-19T08:00:00Z",
        selector: `app=${name}`,
        schedule: null,
        suspended: null,
        lastRun: null,
        ...patch,
    };
}

export const WEB = workload("web");
export const WORKER = workload("worker", { ready: 0, desired: 1, health: "warning", note: "None of 1 ready", images: ["busybox:1.36"] });
export const NIGHTLY = workload("nightly-report", {
    kind: "CronJob",
    ready: null,
    desired: null,
    schedule: "0 2 * * *",
    suspended: false,
    selector: null,
});

export function pod(name: string, patch: Partial<Pod> = {}): Pod {
    return {
        name,
        namespace: "shop",
        status: "Running",
        phase: "Running",
        ready: 1,
        containers: 1,
        restarts: 0,
        created: "2026-10-19T08:00:00Z",
        node: "kind-dev-control-plane",
        ip: "10.244.0.5",
        owner: { kind: "Deployment", name: "web" },
        ...patch,
    };
}

export const WEB_POD = pod("web-56fd9f5c8b-fgprx");
export const WORKER_POD = pod("worker-546f6f965-9swqn", {
    status: "CrashLoopBackOff",
    ready: 0,
    restarts: 6,
    owner: { kind: "Deployment", name: "worker" },
});

export const WORKER_DETAIL: PodDetail = {
    ...WORKER_POD,
    labels: [
        ["app", "worker"],
        ["pod-template-hash", "546f6f965"],
    ],
    conditions: [
        { kind: "PodScheduled", ok: true, reason: null, message: null },
        { kind: "Ready", ok: false, reason: "ContainersNotReady", message: "containers with unready status: [worker]" },
    ],
    containerStates: [
        {
            name: "worker",
            image: "docker.io/library/busybox:1.36",
            init: false,
            ready: false,
            restarts: 6,
            state: "waiting",
            reason: "CrashLoopBackOff",
            message: "back-off 2m40s restarting failed container=worker",
            exitCode: null,
            since: null,
            last: { reason: "Error", exitCode: 1, finished: "2026-10-19T08:40:00Z" },
            ports: [],
            env: [
                { name: "QUEUE_URL", from: "configmap worker-config" },
                { name: "API_TOKEN", from: "secret worker-secrets" },
            ],
            envFrom: [],
            requests: [["cpu", "50m"]],
            limits: [["memory", "64Mi"]],
            probes: [],
        },
    ],
    volumes: [{ name: "kube-api-access-x2", kind: "projected", source: null }],
    events: [
        {
            kind: "Warning",
            reason: "BackOff",
            message: "Back-off restarting failed container worker",
            count: 14,
            last: "2026-10-19T08:41:00Z",
            object: "Pod/worker-546f6f965-9swqn",
            namespace: "shop",
        },
    ],
};

export function node(name: string, patch: Partial<KubeNode> = {}): KubeNode {
    return {
        name,
        roles: [],
        status: "Ready",
        cordoned: false,
        pressure: [],
        reason: null,
        version: "v1.37.0",
        os: "Debian GNU/Linux 13",
        arch: "arm64",
        runtime: "containerd://2.3.4",
        internalIp: "10.0.0.4",
        cpuMillis: 4000,
        memoryBytes: 8 * 1024 ** 3,
        maxPods: 110,
        cpuUsedMillis: null,
        memoryUsedBytes: null,
        taints: [],
        created: "2026-10-19T08:00:00Z",
        ...patch,
    };
}

export const CONTROL = node("kind-dev-control-plane", { roles: ["control-plane"] });
export const SICK = node("kind-dev-worker", {
    status: "NotReady",
    reason: "container runtime network not ready",
    pressure: ["MemoryPressure"],
    cordoned: true,
    taints: ["dedicated=gpu:NoSchedule"],
});

export const WEB_HISTORY: RolloutHistory = {
    kind: "Deployment",
    name: "web",
    paused: false,
    applied: true,
    current: 4,
    revisions: [
        {
            revision: 4,
            name: "web-4",
            current: true,
            changeCause: "pin nginx 1.27.3",
            images: ["nginx:1.27.3-alpine"],
            restart: false,
            created: "2026-10-21T06:15:30Z",
            pods: 2,
        },
        {
            revision: 3,
            name: "web-3",
            current: false,
            changeCause: "move to nginx 1.27",
            images: ["nginx:1.27-alpine"],
            restart: false,
            created: "2026-10-21T06:15:13Z",
            pods: 0,
        },
        {
            revision: 2,
            name: "web-2",
            current: false,
            changeCause: null,
            images: ["nginx:alpine"],
            restart: true,
            created: "2026-10-19T11:54:04Z",
            pods: 0,
        },
        {
            revision: 1,
            name: "web-1",
            current: false,
            changeCause: null,
            images: ["nginx:alpine"],
            restart: false,
            created: "2026-10-19T11:40:21Z",
            pods: 0,
        },
    ],
};

export const STOREFRONT: HelmRelease = {
    name: "storefront",
    namespace: "helm-demo",
    revision: 3,
    updated: "2026-10-21T11:41:22+05:30",
    status: "failed",
    chart: "demo-chart",
    chartVersion: "0.1.0",
    appVersion: "1.16.0",
};

export const STOREFRONT_DETAIL: HelmDetail = {
    history: [
        {
            revision: 3,
            updated: "2026-10-21T11:41:22+05:30",
            status: "failed",
            chart: "demo-chart",
            chartVersion: "0.1.0",
            appVersion: "1.16.0",
            description: "Upgrade failed: context deadline exceeded",
        },
        {
            revision: 2,
            updated: "2026-10-21T11:41:21+05:30",
            status: "deployed",
            chart: "demo-chart",
            chartVersion: "0.1.0",
            appVersion: "1.16.0",
            description: "two replicas for the sale",
        },
        {
            revision: 1,
            updated: "2026-10-21T11:40:35+05:30",
            status: "superseded",
            chart: "demo-chart",
            chartVersion: "0.1.0",
            appVersion: "1.16.0",
            description: "Install complete",
        },
    ],
    made: [
        { kind: "Service", name: "storefront-demo-chart", namespace: null },
        { kind: "Deployment", name: "storefront-demo-chart", namespace: null },
    ],
};
