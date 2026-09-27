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

export const AWS: Record<string, (params: Params) => unknown> = {
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
  ecsClusters: () => [
    {
      name: "prod",
      arn: "arn:aws:ecs:eu-west-1:123456789012:cluster/prod",
      services_count: 5,
      tasks_running: 18,
      tasks_pending: 1,
      status: "ACTIVE",
    },
    {
      name: "staging",
      arn: "arn:aws:ecs:eu-west-1:123456789012:cluster/staging",
      services_count: 5,
      tasks_running: 7,
      tasks_pending: 0,
      status: "ACTIVE",
    },
  ],
  ecsServices: () =>
    SERVICES.map(([name, desired, running], index) => ({
      name,
      arn: `arn:aws:ecs:eu-west-1:123456789012:service/prod/${name}`,
      desired,
      running,
      pending: desired - running,
      status: "ACTIVE",
      primary_created_at: minutesAgo(600 + index * 90),
      primary_updated_at: minutesAgo(40 + index * 55),
    })),
  ecsTasks: () =>
    Array.from({ length: 6 }, (_, index) => {
      const id = `9f${index}c4e1a7b2d44e08a1${index}3f6e2c9d0b${index}`;
      return {
        arn: `arn:aws:ecs:eu-west-1:123456789012:task/prod/${id}`,
        task_id: id,
        status: "RUNNING",
        desired_status: "RUNNING",
        health_status: "HEALTHY",
        cpu: "512",
        memory: "1024",
        started_at: minutesAgo(90 + index * 7),
        last_status_change: minutesAgo(88 + index * 7),
        availability_zone: `eu-west-1${"abc"[index % 3]}`,
        private_ip: `10.0.${12 + index}.${40 + index * 7}`,
      };
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
  billingMonths: () => [
    month(0, 1843.2, true),
    month(1, 2210.75),
    month(2, 2064.1),
    month(3, 1988.4),
    month(4, 1712.9),
  ],
  ec2Instances: () =>
    EC2.map(([name, id, state, type, privateIp, publicIp, days]) => ({
      instance_id: id,
      name,
      state,
      instance_type: type,
      private_ip: privateIp,
      public_ip: publicIp,
      launch_time: minutesAgo(days * 1440),
    })),
  lambdaFunctions: () =>
    LAMBDAS.map(([name, runtime, memory, timeout, days, handler]) => ({
      name,
      runtime,
      memory_size: memory,
      timeout,
      last_modified: minutesAgo(days * 1440),
      handler,
    })),
  sqsQueues: () =>
    QUEUES.map(([name, messages, inFlight, delayed]) => ({
      name,
      url: `https://sqs.eu-west-1.amazonaws.com/123456789012/${name}`,
      messages: String(messages),
      in_flight: String(inFlight),
      delayed: String(delayed),
    })),
  s3Buckets: () =>
    BUCKETS.map(([name, days]) => ({
      name,
      created_at: minutesAgo(days * 1440),
    })),
};

export function awsLogLines(): string[] {
  return LOG_LINES.map(
    (line, index) =>
      `${new Date(Date.now() - (LOG_LINES.length - index) * 4_000).toISOString()} ${line}`,
  );
}
