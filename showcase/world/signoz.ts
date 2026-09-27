type Params = Record<string, unknown>;

const now = Date.now();
let seed = 7;
const random = () => ((seed = (seed * 16807) % 2147483647) - 1) / 2147483646;
const pick = <T>(items: readonly T[]) =>
  items[Math.floor(random() * items.length)];

const SERVICES = [
  ["api-gateway", 0.041, 1900],
  ["auth-service", 0.004, 310],
  ["billing-service", 0.007, 1600],
  ["checkout-web", 0.0004, 870],
  ["search-service", 0, 240],
  ["catalog-service", 0, 180],
  ["notification-service", 0.002, 520],
  ["image-resizer", 0, 2400],
  ["email-worker", 0, 95],
  ["ingest-worker", 0.012, 3100],
] as const;

const services = SERVICES.flatMap(([service, rate, p99], index) =>
  ["production", "staging"].map((environment, env) => {
    const calls = Math.round(18000 / (index + 1) / (env + 1));
    return {
      service,
      environment,
      calls,
      errors: Math.round(calls * rate),
      errorRate: rate,
      p99Ms: p99,
    };
  }),
);

const BODIES = {
  ERROR: [
    "upstream timed out after 30s",
    "payment provider returned 502",
    "request failed validation",
  ],
  WARN: [
    "retrying after 429",
    "slow query took 2.3s table=orders",
    "cache miss storm on catalog:*",
  ],
  INFO: ["request completed", "job finished", "cache refreshed"],
};
const PATHS = ["/v1/checkout", "/v1/search", "/v1/users/me", "/v1/invoices"];

const logLine = (index: number, at: number) => {
  const severity = pick([
    "INFO",
    "INFO",
    "INFO",
    "INFO",
    "WARN",
    "ERROR",
  ] as const);
  return {
    id: `line-${index}`,
    timestamp: new Date(at).toISOString(),
    service: pick([
      "api-gateway",
      "api-gateway",
      "billing-service",
      "auth-service",
    ]),
    severity,
    body: pick(BODIES[severity]),
    traceId: "4bf92f3577b34da6a3ce929d0e0e4736",
    spanId: "00f067aa0ba902b7",
    attributes: {
      method: pick(["GET", "POST"]),
      path: pick(PATHS),
      status: severity === "ERROR" ? 502 : severity === "WARN" ? 429 : 200,
      latency_ms: Math.round(random() * 900),
    },
    resources: {
      "service.name": "api-gateway",
      "deployment.environment": "production",
    },
  };
};

const lines = Array.from({ length: 120 }, (_, index) =>
  logLine(index, now - (120 - index) * 7_000),
);

const minutely = (base: number, spread: number, spikeAt = -1) =>
  Array.from({ length: 60 }, (_, index) => [
    now - (60 - index) * 60_000,
    Math.max(
      0,
      base +
        Math.sin(index / 5) * spread * 0.4 +
        random() * spread +
        (spikeAt >= 0 && index >= spikeAt && index < spikeAt + 6
          ? base * 3
          : 0),
    ),
  ]);

const series = (label: string, base: number, spread: number) => ({
  label,
  points: minutely(base, spread),
});

const panels = [
  {
    id: "v1",
    title: "Requests / min",
    kind: "value",
    unit: "",
    layout: { x: 0, y: 0, w: 3, h: 3 },
    drawable: true,
    query: { id: "rpm" },
  },
  {
    id: "v2",
    title: "Error rate",
    kind: "value",
    unit: "percent",
    layout: { x: 3, y: 0, w: 3, h: 3 },
    drawable: true,
    query: { id: "errors" },
  },
  {
    id: "v3",
    title: "Queue depth",
    kind: "value",
    unit: "",
    layout: { x: 6, y: 0, w: 3, h: 3 },
    drawable: true,
    query: { id: "queue" },
  },
  {
    id: "v4",
    title: "Active workers",
    kind: "value",
    unit: "",
    layout: { x: 9, y: 0, w: 3, h: 3 },
    drawable: true,
    query: { id: "workers" },
  },
  {
    id: "g1",
    title: "Throughput by service",
    kind: "graph",
    unit: "",
    layout: { x: 0, y: 3, w: 6, h: 7 },
    drawable: true,
    query: { id: "throughput" },
  },
  {
    id: "g2",
    title: "p99 latency",
    kind: "graph",
    unit: "ms",
    layout: { x: 6, y: 3, w: 6, h: 7 },
    drawable: true,
    query: { id: "latency" },
  },
  {
    id: "t1",
    title: "Slowest endpoints",
    kind: "table",
    unit: "ms",
    layout: { x: 0, y: 10, w: 6, h: 6 },
    drawable: true,
    query: { id: "slowest" },
  },
  {
    id: "p1",
    title: "Jobs by type",
    kind: "pie",
    unit: "",
    layout: { x: 6, y: 10, w: 6, h: 6 },
    drawable: true,
    query: { id: "jobs" },
  },
];

const panelData: Record<string, unknown> = {
  rpm: { shape: "value", value: 1284 },
  errors: { shape: "value", value: 2.37 },
  queue: { shape: "value", value: 42 },
  workers: { shape: "value", value: 12 },
  throughput: {
    shape: "series",
    series: [
      series("api-gateway", 40, 12),
      series("billing-service", 22, 8),
      series("search-service", 12, 5),
      series("ingest-worker", 6, 3),
    ],
  },
  latency: {
    shape: "series",
    series: [series("GET", 120, 40), series("POST", 80, 30)],
  },
  slowest: {
    shape: "table",
    columns: [
      { name: "http.route", aggregation: false },
      { name: "p99", aggregation: true },
    ],
    rows: [
      ["/v1/checkout", 2210],
      ["/v1/invoices", 1630],
      ["/v1/users/me", 412],
      ["/v1/search", 188],
    ],
  },
  jobs: {
    shape: "table",
    columns: [
      { name: "job_type", aggregation: false },
      { name: "count", aggregation: true },
    ],
    rows: [
      ["resize", 420],
      ["email", 188],
      ["ingest", 96],
      ["export", 40],
    ],
  },
};

const status = {
  configured: true,
  url: "https://signoz.acme.dev",
  auth: "session",
  email: "edon@acme.dev",
  keyFromEnvironment: false,
  version: "v0.114.1",
  ok: true,
  authFailed: false,
  message: null,
};

export const SIGNOZ: Record<string, (params: Params) => unknown> = {
  status: () => status,
  services: () => services,
  serviceOverview: () => ({
    calls: 72_300,
    errors: 2_960,
    errorRate: 0.041,
    perMinute: 1_205,
    p99Ms: 1_900,
    p50Ms: 41,
    requests: minutely(1_150, 160),
    failures: minutely(40, 20, 44),
    p99: minutely(1_400, 300, 44),
  }),
  operations: () => [
    {
      name: "POST /v1/checkout",
      calls: 18_600,
      errors: 1_480,
      errorRate: 0.08,
      p99Ms: 2_210,
      p50Ms: 320,
    },
    {
      name: "GET /v1/search",
      calls: 32_400,
      errors: 240,
      errorRate: 0.007,
      p99Ms: 190,
      p50Ms: 44,
    },
    {
      name: "GET /v1/users/me",
      calls: 13_200,
      errors: 0,
      errorRate: 0,
      p99Ms: 44,
      p50Ms: 6,
    },
    {
      name: "GET /health",
      calls: 5_700,
      errors: 0,
      errorRate: 0,
      p99Ms: 2,
      p50Ms: 1,
    },
  ],
  errorGroups: () => [
    {
      pattern: "upstream timed out after <n>s",
      sample: "upstream timed out after 30s",
      count: 412,
      firstSeen: now - 50 * 60_000,
      lastSeen: now - 60_000,
    },
    {
      pattern: "payment provider returned <n>",
      sample: "payment provider returned 502",
      count: 188,
      firstSeen: now - 58 * 60_000,
      lastSeen: now - 3 * 60_000,
    },
    {
      pattern: "retrying after <n>",
      sample: "retrying after 429",
      count: 73,
      firstSeen: now - 59 * 60_000,
      lastSeen: now - 9 * 60_000,
    },
  ],
  searchLogs: () => ({ lines: lines.slice(-80).reverse(), nextOffset: 200 }),
  logVolume: () =>
    Array.from({ length: 60 }, (_, index) => ({
      start: now - (60 - index) * 15_000,
      counts: {
        ERROR: Math.round(random() * 6),
        WARN: Math.round(random() * 8),
        INFO: Math.round(20 + random() * 40),
      },
    })),
  searchTraces: () => ({
    traces: Array.from({ length: 30 }, (_, index) => ({
      traceId: `4bf92f3577b34da6a3ce929d0e0e${String(index).padStart(4, "0")}`,
      timestamp: new Date(now - index * 20_000).toISOString(),
      service: pick(["api-gateway", "billing-service", "search-service"]),
      name: pick(["POST /v1/checkout", "GET /v1/users/me", "GET /v1/search"]),
      durationMs: 3000 / (index + 1),
      error: index % 7 === 3,
      statusCode: index % 7 === 3 ? "502" : "200",
    })),
    nextOffset: 100,
  }),
  dashboards: () => [
    {
      id: "dash-api",
      title: "API overview",
      description: "Rate, errors and latency across the edge",
      tags: ["api"],
      panels: 8,
    },
    {
      id: "dash-workers",
      title: "Workers",
      description: "Queues, concurrency and throughput",
      tags: ["workers"],
      panels: 6,
    },
  ],
  dashboard: () => ({
    id: "dash-api",
    title: "API overview",
    variables: [
      {
        name: "environment",
        options: ["staging", "production"],
        selected: "production",
      },
    ],
    panels,
  }),
  panel: (request) => panelData[(request.query as { id: string }).id],
  fieldKeys: () => [
    { name: "path", context: "attribute", dataType: "string" },
    { name: "status", context: "attribute", dataType: "number" },
  ],
  fieldValues: () => PATHS,
};

export const signozTail = () => ({ lines: lines.slice(-60), error: null });
