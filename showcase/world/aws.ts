type Params = Record<string, unknown>;

const minutesAgo = (minutes: number) =>
  new Date(Date.now() - minutes * 60_000).toISOString();

const SERVICES = [
  ["api-gateway", 6, 6],
  ["billing-service", 4, 4],
  ["checkout-web", 3, 3],
  ["search-service", 4, 3],
  ["ingest-worker", 2, 2],
] as const;

const LOG_LINES = [
  "INFO  request completed method=GET path=/v1/search status=200 latency_ms=38",
  "INFO  request completed method=POST path=/v1/checkout status=200 latency_ms=212",
  "WARN  retrying after 429 upstream=payments attempt=2",
  "INFO  request completed method=GET path=/v1/users/me status=200 latency_ms=6",
  "INFO  cache refreshed key=catalog:featured entries=48",
  "ERROR upstream timed out after 30s upstream=payments path=/v1/checkout",
  "INFO  request completed method=POST path=/v1/checkout status=200 latency_ms=198",
  "INFO  request completed method=GET path=/v1/search status=200 latency_ms=41",
  "INFO  health check ok uptime=6d4h",
  "INFO  request completed method=GET path=/v1/invoices status=200 latency_ms=77",
];

const REGION = "eu-west-1";
const CONSOLE = `https://${REGION}.console.aws.amazon.com`;

const EC2 = [
  ["bastion", "i-0a41f93c2d7e81b05", "running", "t3.micro", "10.0.1.24", "34.242.18.201", 120],
  ["ci-runner-1", "i-07c2e5b1af3d90c44", "running", "c6i.xlarge", "10.0.2.61", null, 21],
  ["ci-runner-2", "i-0e61c0b9f2d4a8173", "running", "c6i.xlarge", "10.0.2.62", null, 21],
  ["metrics-host", "i-0f19d8ce04a6b7231", "running", "m6i.large", "10.0.3.18", null, 64],
  ["redis-sentinel", "i-09a6e3f1c7d2b8540", "running", "r6g.large", "10.0.5.33", null, 150],
  ["ml-worker", "i-0c94a1e7b3f5d2860", "stopped", "g5.xlarge", "10.0.14.12", null, 48],
  ["legacy-api", "i-01b7d4c9e2a6f3875", "stopped", "t2.medium", "10.0.4.90", null, 410],
] as const;

const LAMBDAS = [
  ["image-resizer", "nodejs20.x", 1024, 30, 2, "index.handler"],
  ["auth-authorizer", "nodejs20.x", 256, 5, 19, "authorizer.handler"],
  ["webhook-ingest", "python3.12", 512, 15, 1, "app.lambda_handler"],
  ["report-export", "python3.12", 2048, 900, 33, "export.main"],
  ["email-render", "nodejs18.x", 512, 60, 5, "render.handler"],
  ["invoice-pdf", "java21", 3008, 120, 12, "com.acme.Invoice::handle"],
  ["cost-alert", "python3.9", 128, 30, 240, "alert.handler"],
] as const;

const QUEUES = [
  ["orders", 142, 18, 0],
  ["orders-dlq", 7, 0, 0],
  ["notifications", 0, 3, 0],
  ["events.fifo", 1204, 64, 12],
  ["webhooks", 38, 9, 0],
  ["webhooks-dlq", 0, 0, 0],
  ["email-send", 5210, 200, 0],
] as const;

const BUCKETS = [
  ["acme-assets-prod", 1180],
  ["acme-assets-staging", 1180],
  ["acme-user-uploads", 1120],
  ["acme-db-backups", 1030],
  ["acme-alb-logs", 900],
  ["acme-exports", 640],
  ["terraform-state-acme", 1260],
] as const;

const month = (offset: number, total: number, current = false) => {
  const start = new Date(2026, 8 - offset, 1);
  const end = new Date(2026, 9 - offset, 1);
  const share = [0.46, 0.21, 0.12, 0.09, 0.07, 0.05];
  const names = [
    "Amazon Elastic Container Service",
    "Amazon Relational Database Service",
    "Amazon Elastic Compute Cloud",
    "Amazon Simple Storage Service",
    "AWS Lambda",
    "Amazon CloudWatch",
  ];
  return {
    period_start: start.toISOString().slice(0, 10),
    period_end: end.toISOString().slice(0, 10),
    total: total.toFixed(2),
    unit: "USD",
    is_current: current,
    by_service: names.map((service, index) => ({
      service,
      amount: (total * share[index]).toFixed(2),
      unit: "USD",
    })),
  };
};

let paneProfile: string | null = null;

export const AWS: Record<string, (params: Params) => unknown> = {
  profile: () => ({ profile: paneProfile }),
  setDefaultAccount: (params) => {
    paneProfile = typeof params.profile === "string" ? params.profile : null;
    return { profile: paneProfile };
  },
  status: () => ({
    configured: paneProfile !== null,
    ok: paneProfile !== null,
    authFailed: false,
    message: null,
    profile: paneProfile,
    account: paneProfile && "123456789012",
  }),
  profiles: () => [
    {
      name: "acme-prod",
      region: "eu-west-1",
      sso_start_url: "https://acme.awsapps.com/start",
      sso_region: "eu-west-1",
      sso_account_id: "123456789012",
      sso_role_name: "Developer",
      kind: "sso",
    },
  ],
  identity: () => ({
    arn: "arn:aws:sts::123456789012:assumed-role/Developer/edon",
    account: "123456789012",
    user_id: "AROAEXAMPLE:edon",
    status: "authed",
    message: null,
  }),
  agentProfileView: () => ({
    project: null,
    default: null,
    effective: "acme-prod",
    fromShell: null,
    projectUnset: "Default",
    defaultUnset: "Automatic (acme-prod)",
    profiles: ["acme-prod"],
  }),
  ecsClustersView: () => ({
    region: REGION,
    envs: [
      { key: "staging", label: "Staging", count: 1 },
      { key: "prod", label: "Prod", count: 1 },
    ],
    clusters: [
      ["prod", 18, 1],
      ["staging", 7, 0],
    ].map(([name, running, pending]) => ({
      name,
      shortName: name,
      arn: `arn:aws:ecs:${REGION}:123456789012:cluster/${name}`,
      product: name,
      env: name,
      envLabel: name,
      production: name === "prod",
      services: 5,
      running,
      pending,
      health: pending ? "warn" : "ok",
      consoleUrl: `${CONSOLE}/ecs/v2/clusters/${name}/services?region=${REGION}`,
    })),
  }),
  ecsServicesView: ({ cluster }) => ({
    region: REGION,
    cluster,
    services: SERVICES.map(([name, desired, running], index) => ({
      name,
      arn: `arn:aws:ecs:${REGION}:123456789012:service/prod/${name}`,
      desired,
      running,
      pending: desired - running,
      health: desired === running ? "ok" : "warn",
      stateLabel: desired === running ? "Steady" : "Deploying",
      deployedAt: minutesAgo(40 + index * 55),
      consoleUrl: `${CONSOLE}/ecs/v2/clusters/${String(cluster)}/services/${name}/health?region=${REGION}`,
    })),
  }),
  ecsTasksView: () => ({
    tasks: Array.from({ length: 6 }, (_, index) => {
      const id = `9f${index}c4e1a7b2d44e08a1${index}3f6e2c9d0b${index}`;
      return {
        arn: `arn:aws:ecs:${REGION}:123456789012:task/prod/${id}`,
        id,
        shortId: id.slice(0, 12),
        idRest: id.slice(12, 20),
        health: "ok",
        statusLabel: "Running",
        check: "ok",
        checkLabel: "Healthy",
        privateIp: `10.0.${12 + index}.${40 + index * 7}`,
        zone: `${REGION}${"abc"[index % 3]}`,
        size: "0.5 vCPU · 1 GB",
        startedAt: minutesAgo(90 + index * 7),
      };
    }),
  }),
  ecsServiceLogConfig: ({ service }) => ({
    log_group: `/ecs/prod/${String(service)}`,
    container_name: String(service),
    region: "eu-west-1",
  }),
  ecsTaskLogConfig: () => ({
    log_group: "/ecs/prod/api-gateway",
    log_stream: "api-gateway/api-gateway/9f0c4e1a",
    container_name: "api-gateway",
    region: "eu-west-1",
  }),
  billingView: () => billing(),
  ec2View: () => ({
    region: REGION,
    running: EC2.filter(([, , state]) => state === "running").length,
    instances: EC2.map(([name, id, state, type, privateIp, publicIp, days]) => ({
      id,
      name,
      state,
      stateLabel: state === "running" ? "Running" : "Stopped",
      health: state === "running" ? "ok" : "off",
      idle: state !== "running",
      instanceType: type,
      privateIp,
      publicIp,
      launchedAt: minutesAgo(days * 1440),
      consoleUrl: `${CONSOLE}/ec2/home?region=${REGION}#InstanceDetails:instanceId=${id}`,
    })),
  }),
  lambdaView: () => ({
    region: REGION,
    families: [
      { key: "nodejs", label: "Node", count: 3 },
      { key: "python", label: "Python", count: 3 },
      { key: "java", label: "Java", count: 1 },
    ],
    functions: LAMBDAS.map(([name, runtime, memory, timeout, days, handler]) => ({
      name,
      runtime,
      family: runtime.replace(/[\d.]+x?$/, ""),
      deprecated: runtime === "nodejs18.x" || runtime === "python3.9",
      memoryMb: memory,
      memoryLabel: memory >= 1024 ? `${+(memory / 1024).toFixed(1)} GB` : `${memory} MB`,
      memoryRatio: memory / 3008,
      timeoutSecs: timeout,
      handler,
      modifiedAt: minutesAgo(days * 1440),
      logGroup: `/aws/lambda/${name}`,
      consoleUrl: `${CONSOLE}/lambda/home?region=${REGION}#/functions/${name}`,
    })),
  }),
  sqsView: () => ({
    region: REGION,
    deadLetters: QUEUES.filter(([name]) => name.endsWith("-dlq")).length,
    queues: QUEUES.map(([name, available, inFlight, delayed]) => {
      const url = `https://sqs.${REGION}.amazonaws.com/123456789012/${name}`;
      const deadLetter = name.endsWith("-dlq");
      return {
        name,
        url,
        fifo: name.endsWith(".fifo"),
        deadLetter,
        available,
        inFlight,
        delayed,
        ratio: available ? Math.max(0.03, Math.sqrt(available / 5210)) : 0,
        needsLook: deadLetter && available > 0,
        consoleUrl: `${CONSOLE}/sqs/v3/home?region=${REGION}#/queues/${encodeURIComponent(url)}`,
      };
    }),
  }),
  s3View: () => ({
    region: REGION,
    buckets: BUCKETS.map(([name, days]) => ({
      name,
      createdAt: minutesAgo(days * 1440),
      uri: `s3://${name}`,
      arn: `arn:aws:s3:::${name}`,
      consoleUrl: `https://s3.console.aws.amazon.com/s3/buckets/${name}`,
    })),
  }),
};

/** The costs view the plugin works out, for the months above, oldest first. */
function billing() {
  const totals = [1843.2, 2210.75, 2064.1, 1988.4, 1712.9];
  const months = [4, 3, 2, 1, 0].map((offset) => month(offset, totals[offset], offset === 0));
  const tallest = Math.max(...months.map((m) => Number(m.total)));
  const name = (start: string) => new Date(`${start}T00:00:00Z`).toLocaleString("en-US", { month: "long", timeZone: "UTC" });
  const label = (start: string) => `${name(start).slice(0, 3)} ${start.slice(2, 4)}`;
  const shaped = months.map((m, index) => {
    const gross = Number(m.total);
    const before = months[index - 1];
    const charges = m.by_service.map((line) => {
      const amount = Number(line.amount);
      const was = Number(before?.by_service.find((b) => b.service === line.service)?.amount);
      const pct = before ? ((amount - was) / was) * 100 : null;
      return {
        service: line.service,
        shortName: line.service.replace(/^(Amazon|AWS) /, ""),
        amount,
        ratio: amount / Number(m.by_service[0].amount),
        change: pct === null ? null : Math.abs(pct) < 1 ? { kind: "flat" } : pct > 0 ? { kind: "up", pct } : { kind: "down", pct },
      };
    });
    return {
      periodStart: m.period_start,
      label: label(m.period_start),
      name: name(m.period_start),
      longLabel: `${name(m.period_start)} ${m.period_start.slice(0, 4)}`,
      inProgress: m.is_current,
      day: m.is_current ? 18 : null,
      days: 30,
      gross,
      credits: 0,
      net: gross,
      ratio: gross / tallest,
      comparedTo: before ? label(before.period_start) : null,
      charges,
      creditLines: [],
    };
  });
  const closed = shaped.slice(0, -1);
  const average = closed.reduce((sum, m) => sum + m.gross, 0) / closed.length;
  const forecast = (shaped[shaped.length - 1].gross / 18) * 30;
  const last = closed[closed.length - 1].gross;
  return {
    unit: "USD",
    months: shaped,
    current: shaped.length - 1,
    forecast,
    lastClosed: closed.length - 1,
    changePct: ((forecast - last) / last) * 100,
    average,
    averageRatio: average / tallest,
  };
}

export function awsLogLines() {
  return LOG_LINES.map((text, index) => {
    const at = new Date(Date.now() - (LOG_LINES.length - index) * 4_000).toISOString();
    const level = text.startsWith("ERROR") ? "error" : text.startsWith("WARN") ? "warn" : null;
    return { day: at.slice(0, 10), clock: at.slice(11, 19), text, level };
  });
}
