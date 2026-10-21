type Params = Record<string, unknown>;

const ago = (minutes: number) =>
  new Date(Date.now() - minutes * 60_000).toISOString();

const CONTEXTS = [
  {
    name: "gke_acme_prod",
    cluster: "gke_acme-prod_europe-west1_prod",
    server: "https://34.76.18.201",
    namespace: "shop",
  },
  {
    name: "gke_acme_staging",
    cluster: "gke_acme-staging_europe-west1_staging",
    server: "https://35.205.40.17",
    namespace: "shop",
  },
  {
    name: "kind-dev",
    cluster: "kind-dev",
    server: "https://127.0.0.1:6443",
    namespace: null,
  },
];

const NAMESPACES = ["default", "shop", "payments", "monitoring", "kube-system"];

const workload = (name: string, patch: Params = {}) => ({
  kind: "Deployment",
  name,
  namespace: "shop",
  ready: 3,
  desired: 3,
  health: "ok",
  note: null,
  images: [`europe-docker.pkg.dev/acme/shop/${name}:4.12.0`],
  created: ago(60 * 24 * 9),
  selector: `app=${name}`,
  schedule: null,
  suspended: null,
  lastRun: null,
  ...patch,
});

const WORKLOADS = [
  workload("storefront", { ready: 3, desired: 3 }),
  workload("checkout-api", {
    ready: 2,
    desired: 3,
    health: "progressing",
    note: "Rolling out 2 of 3 new pods",
    images: ["europe-docker.pkg.dev/acme/shop/checkout-api:4.13.0"],
  }),
  workload("order-worker", {
    ready: 0,
    desired: 2,
    health: "warning",
    note: "None of 2 ready",
    images: ["europe-docker.pkg.dev/acme/shop/order-worker:4.13.0"],
  }),
  workload("search", { ready: 2, desired: 2 }),
  workload("redis", {
    kind: "StatefulSet",
    ready: 3,
    desired: 3,
    images: ["redis:7.2-alpine"],
    created: ago(60 * 24 * 40),
  }),
  workload("node-exporter", {
    kind: "DaemonSet",
    ready: 4,
    desired: 4,
    images: ["quay.io/prometheus/node-exporter:v1.8.1"],
  }),
  workload("nightly-report", {
    kind: "CronJob",
    ready: null,
    desired: null,
    schedule: "0 2 * * *",
    suspended: false,
    lastRun: ago(60 * 8),
    selector: null,
    images: ["europe-docker.pkg.dev/acme/shop/reports:1.4.0"],
  }),
  workload("cache-warm", {
    kind: "CronJob",
    ready: null,
    desired: null,
    schedule: "*/30 * * * *",
    suspended: true,
    lastRun: ago(60 * 26),
    selector: null,
    images: ["europe-docker.pkg.dev/acme/shop/cache-warm:0.9.2"],
  }),
];

const NODES = [
  "gke-prod-pool-1-7f3a",
  "gke-prod-pool-1-91c2",
  "gke-prod-pool-2-0bd4",
  "gke-prod-pool-2-e65f",
];

const pod = (
  name: string,
  owner: string,
  index: number,
  patch: Params = {},
) => ({
  name,
  namespace: "shop",
  status: "Running",
  phase: "Running",
  ready: 1,
  containers: 1,
  restarts: 0,
  created: ago(60 * 20 + index * 7),
  node: NODES[index % NODES.length],
  ip: `10.44.${index + 1}.${12 + index * 3}`,
  owner: {
    kind: owner === "redis" ? "StatefulSet" : "Deployment",
    name: owner,
  },
  ...patch,
});

const PODS = [
  pod("order-worker-6c9d8b7f5-q2kxm", "order-worker", 0, {
    status: "CrashLoopBackOff",
    ready: 0,
    restarts: 14,
    created: ago(48),
  }),
  pod("order-worker-6c9d8b7f5-z8wpt", "order-worker", 1, {
    status: "CrashLoopBackOff",
    ready: 0,
    restarts: 13,
    created: ago(48),
  }),
  pod("checkout-api-84f6d9c5d-hk3vn", "checkout-api", 2, {
    status: "ContainerCreating",
    phase: "Pending",
    ready: 0,
    created: ago(1),
  }),
  pod("checkout-api-84f6d9c5d-7tqlr", "checkout-api", 3, { created: ago(4) }),
  pod("checkout-api-84f6d9c5d-m5bxs", "checkout-api", 0, { created: ago(6) }),
  pod("storefront-7d8c9f6b4-2lvwp", "storefront", 1, {
    ready: 2,
    containers: 2,
  }),
  pod("storefront-7d8c9f6b4-9rjqd", "storefront", 2, {
    ready: 2,
    containers: 2,
  }),
  pod("storefront-7d8c9f6b4-xm4ct", "storefront", 3, {
    ready: 2,
    containers: 2,
    restarts: 1,
  }),
  pod("search-5b7f8c6d9-fq2hn", "search", 0),
  pod("search-5b7f8c6d9-vk7zr", "search", 1),
  pod("redis-0", "redis", 2, { created: ago(60 * 24 * 12) }),
  pod("redis-1", "redis", 3, { created: ago(60 * 24 * 12) }),
  pod("redis-2", "redis", 0, { created: ago(60 * 24 * 12) }),
  {
    ...pod("nightly-report-29071440-8xk2c", "nightly-report", 1, {
      status: "Completed",
      phase: "Succeeded",
      ready: 0,
      created: ago(60 * 8),
    }),
    owner: { kind: "Job", name: "nightly-report-29071440" },
  },
];

const EVENTS = [
  {
    kind: "Warning",
    reason: "BackOff",
    message:
      "Back-off restarting failed container worker in pod order-worker-6c9d8b7f5-q2kxm",
    count: 212,
    last: ago(0.5),
    object: "Pod/order-worker-6c9d8b7f5-q2kxm",
  },
  {
    kind: "Warning",
    reason: "BackOff",
    message:
      "Back-off restarting failed container worker in pod order-worker-6c9d8b7f5-z8wpt",
    count: 198,
    last: ago(1),
    object: "Pod/order-worker-6c9d8b7f5-z8wpt",
  },
  {
    kind: "Warning",
    reason: "FailedScheduling",
    message:
      "0/4 nodes are available: 4 Insufficient memory. preemption: 0/4 nodes are available.",
    count: 3,
    last: ago(2),
    object: "Pod/checkout-api-84f6d9c5d-hk3vn",
  },
  {
    kind: "Normal",
    reason: "ScalingReplicaSet",
    message: "Scaled up replica set checkout-api-84f6d9c5d to 3 from 2",
    count: 1,
    last: ago(6),
    object: "Deployment/checkout-api",
  },
  {
    kind: "Normal",
    reason: "Pulled",
    message:
      'Container image "europe-docker.pkg.dev/acme/shop/checkout-api:4.13.0" already present on machine',
    count: 2,
    last: ago(4),
    object: "Pod/checkout-api-84f6d9c5d-7tqlr",
  },
  {
    kind: "Normal",
    reason: "SuccessfulCreate",
    message: "Created job nightly-report-29071440",
    count: 1,
    last: ago(60 * 8),
    object: "CronJob/nightly-report",
  },
];

const SERVICES = [
  {
    name: "storefront",
    namespace: "shop",
    kind: "ClusterIP",
    clusterIp: "10.48.3.17",
    external: [],
    ports: ["80→3000/TCP"],
    selector: "app=storefront",
  },
  {
    name: "checkout-api",
    namespace: "shop",
    kind: "ClusterIP",
    clusterIp: "10.48.7.102",
    external: [],
    ports: ["80→8080/TCP", "9090/TCP"],
    selector: "app=checkout-api",
  },
  {
    name: "search",
    namespace: "shop",
    kind: "ClusterIP",
    clusterIp: "10.48.11.40",
    external: [],
    ports: ["9200/TCP"],
    selector: "app=search",
  },
  {
    name: "redis",
    namespace: "shop",
    kind: "ClusterIP",
    clusterIp: null,
    external: [],
    ports: ["6379/TCP"],
    selector: "app=redis",
  },
  {
    name: "edge",
    namespace: "shop",
    kind: "LoadBalancer",
    clusterIp: "10.48.0.9",
    external: ["34.140.92.18"],
    ports: ["443→8443/TCP (node 31443)"],
    selector: "app=storefront",
  },
];

const INGRESSES = [
  {
    name: "shop",
    namespace: "shop",
    class: "gce",
    tls: ["shop.acme.dev", "api.acme.dev"],
    address: ["34.120.55.7"],
    rules: [
      { host: "shop.acme.dev", path: "/", service: "storefront", port: "80" },
      {
        host: "api.acme.dev",
        path: "/checkout",
        service: "checkout-api",
        port: "80",
      },
      {
        host: "api.acme.dev",
        path: "/search",
        service: "search",
        port: "9200",
      },
    ],
  },
];

const CONFIG = {
  configMaps: [
    {
      name: "checkout-config",
      namespace: "shop",
      kind: null,
      keys: [
        { name: "PAYMENTS_URL", size: 38 },
        { name: "RETRY_LIMIT", size: 1 },
        { name: "FEATURE_FLAGS", size: 412 },
      ],
    },
    {
      name: "storefront-config",
      namespace: "shop",
      kind: null,
      keys: [
        { name: "API_URL", size: 24 },
        { name: "CDN_HOST", size: 19 },
      ],
    },
    {
      name: "worker-config",
      namespace: "shop",
      kind: null,
      keys: [
        { name: "QUEUE_URL", size: 44 },
        { name: "CONCURRENCY", size: 1 },
      ],
    },
  ],
  secrets: [
    {
      name: "checkout-secrets",
      namespace: "shop",
      kind: "Opaque",
      keys: [
        { name: "STRIPE_SECRET_KEY", size: 144 },
        { name: "DATABASE_URL", size: 120 },
      ],
    },
    {
      name: "shop-acme-dev-tls",
      namespace: "shop",
      kind: "kubernetes.io/tls",
      keys: [
        { name: "tls.crt", size: 3880 },
        { name: "tls.key", size: 2272 },
      ],
    },
    {
      name: "worker-secrets",
      namespace: "shop",
      kind: "Opaque",
      keys: [{ name: "QUEUE_TOKEN", size: 64 }],
    },
  ],
};

const container = (name: string, image: string, patch: Params = {}) => ({
  name,
  image,
  init: false,
  ready: true,
  restarts: 0,
  state: "running",
  reason: null,
  message: null,
  exitCode: null,
  since: ago(20),
  last: null,
  ports: ["8080/TCP"],
  env: [],
  envFrom: [],
  requests: [
    ["cpu", "100m"],
    ["memory", "256Mi"],
  ],
  limits: [["memory", "512Mi"]],
  probes: [
    "readiness GET :8080/healthz every 10s",
    "liveness GET :8080/healthz every 20s",
  ],
  ...patch,
});

function detailOf(name: string) {
  const found = PODS.find((each) => each.name === name);
  if (!found) return null;
  const owner = found.owner.name;
  const image =
    WORKLOADS.find((each) => each.name === owner)?.images[0] ??
    "europe-docker.pkg.dev/acme/shop/reports:1.4.0";
  const crashing = found.status === "CrashLoopBackOff";
  const pending = found.status === "ContainerCreating";
  return {
    ...found,
    labels: [
      ["app", owner],
      ["pod-template-hash", name.split("-").at(-2) ?? ""],
      ["team", "commerce"],
    ],
    conditions: [
      {
        kind: "PodScheduled",
        ok: !pending,
        reason: pending ? "Unschedulable" : null,
        message: null,
      },
      { kind: "Initialized", ok: true, reason: null, message: null },
      {
        kind: "Ready",
        ok: found.ready >= found.containers,
        reason: found.ready >= found.containers ? null : "ContainersNotReady",
        message:
          found.ready >= found.containers
            ? null
            : `containers with unready status: [${crashing ? "worker" : owner}]`,
      },
    ],
    containerStates: crashing
      ? [
          container("worker", image, {
            ready: false,
            restarts: found.restarts,
            state: "waiting",
            reason: "CrashLoopBackOff",
            message: `back-off 5m0s restarting failed container=worker pod=${name}`,
            since: null,
            last: { reason: "Error", exitCode: 1, finished: ago(1.5) },
            ports: [],
            env: [
              { name: "QUEUE_URL", from: "configmap worker-config" },
              { name: "QUEUE_TOKEN", from: "secret worker-secrets" },
              { name: "CHECKOUT_URL", from: "set here" },
            ],
            envFrom: [],
            probes: [],
          }),
        ]
      : pending
        ? [
            container(owner, image, {
              ready: false,
              state: "waiting",
              reason: "ContainerCreating",
              since: null,
            }),
          ]
        : found.containers === 2
          ? [
              container(owner, image, {
                restarts: found.restarts,
                ports: ["3000/TCP"],
                envFrom: ["configmap storefront-config"],
              }),
              container("envoy", "envoyproxy/envoy:v1.30.1", {
                ports: ["9901/TCP"],
                probes: [],
              }),
            ]
          : [
              container(owner, image, {
                envFrom: [`configmap ${owner}-config`],
              }),
            ],
    volumes: [
      { name: "kube-api-access-4v9tq", kind: "projected", source: null },
      ...(owner === "redis"
        ? [
            {
              name: "data",
              kind: "persistentVolumeClaim",
              source: `data-${name}`,
            },
          ]
        : []),
    ],
    events: EVENTS.filter((event) => event.object === `Pod/${name}`),
  };
}

const WORKER_LOGS = [
  'level=info msg="order-worker 4.13.0 starting" concurrency=8',
  'level=info msg="connecting to queue" url=amqps://queue.shop.svc:5671/orders',
  'level=info msg="connected" consumers=8',
  'level=info msg="claimed order" order=ord_8f2k1 attempt=1',
  'level=error msg="checkout api answered 404" path=/v2/orders/ord_8f2k1/capture',
  'level=error msg="capture failed" order=ord_8f2k1 err="POST http://checkout-api/v2/orders/ord_8f2k1/capture: 404 Not Found"',
  "panic: capture failed for ord_8f2k1: unexpected status 404",
  "",
  "goroutine 41 [running]:",
  "main.(*Worker).capture(0xc0001b2000, {0xc000412060, 0x9})",
  "\t/src/worker/capture.go:88 +0x2f4",
  "main.(*Worker).handle(0xc0001b2000, 0xc0003a8000)",
  "\t/src/worker/worker.go:131 +0x1a5",
];

const appLogs = (name: string) =>
  Array.from({ length: 80 }, (_, index) => {
    const paths = [
      "/",
      "/products/sneakers",
      "/cart",
      "/api/search?q=boots",
      "/healthz",
    ];
    const path = paths[index % paths.length];
    const ms = 4 + ((index * 37) % 120);
    return `${new Date(Date.now() - (80 - index) * 4_000).toISOString()} ${name} GET ${path} 200 ${ms}ms`;
  });

function logsOf(name: string, previous: boolean) {
  const crashing = name.startsWith("order-worker");
  const lines = crashing
    ? WORKER_LOGS.map((text, index) => ({
        at: new Date(
          Date.now() - (previous ? 150_000 : 20_000) + index * 40,
        ).toISOString(),
        text,
      }))
    : appLogs(name.split("-")[0]).map((line) => ({
        at: line.slice(0, 24),
        text: line.slice(25),
      }));
  return { lines, filtered: 0 };
}

const inScope = <T extends { namespace: string }>(
  items: T[],
  params: Params,
) =>
  params.allNamespaces || !params.namespace || params.namespace === "shop"
    ? items
    : [];

function podsFor(params: Params) {
  const selector = params.selector as string | undefined;
  const app = selector?.split("=")[1];
  const found = inScope(PODS, params).filter(
    (each) => !app || each.owner.name === app,
  );
  const rank = (status: string) =>
    /BackOff|Err/.test(status)
      ? 0
      : status === "Running"
        ? 2
        : status === "Completed"
          ? 3
          : 1;
  return [...found].sort((a, b) => rank(a.status) - rank(b.status));
}

export const KUBERNETES: Record<string, (params: Params) => unknown> = {
  status: () => ({
    ready: true,
    message: null,
    category: null,
    current: "gke_acme_prod",
    contexts: CONTEXTS,
  }),
  namespaces: () => NAMESPACES.map((name) => ({ name, phase: "Active" })),
  workloads: (params) => inScope(WORKLOADS, params),
  pods: podsFor,
  pod: (params) => detailOf(params.name as string),
  logs: (params) => logsOf(params.pod as string, Boolean(params.previous)),
  events: (params) =>
    inScope(
      EVENTS.map((event) => ({ ...event, namespace: "shop" })),
      params,
    ),
  network: (params) => ({
    services: inScope(SERVICES, params),
    ingresses: inScope(INGRESSES, params),
  }),
  config: (params) => ({
    configMaps: inScope(CONFIG.configMaps, params),
    secrets: inScope(CONFIG.secrets, params),
  }),
  act: (params) => ({
    message: `${String(params.kind ?? "deployment").toLowerCase()}.apps/${String(params.name)} ${params.action === "scale" ? "scaled" : "restarted"}`,
  }),
};

export function kubernetesFollow(params: Params) {
  const { lines } = logsOf(params.pod as string, false);
  return { lines, ended: false };
}
