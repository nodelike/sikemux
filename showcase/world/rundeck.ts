type Params = Record<string, unknown>;

const PROJECT = "platform";
const SERVICES = [
  "api-gateway",
  "auth-service",
  "billing-service",
  "checkout-web",
  "search-service",
  "notification-service",
  "ingest-worker",
];
const ENVS = ["staging", "production"];
const BRANCHES = [
  "main",
  "main",
  "release/2.14",
  "main",
  "feat/search-rerank",
  "main",
  "main",
];
const USERS = [
  "edon",
  "maya",
  "deploy-bot",
  "edon",
  "sam",
  "maya",
  "deploy-bot",
];

const iso = (minutesAgo: number) =>
  new Date(Date.now() - minutesAgo * 60_000).toISOString();

const jobId = (service: string, env: string) => `job-${env}-${service}`;

const jobs = ENVS.flatMap((env) =>
  SERVICES.map((service) => ({
    id: jobId(service, env),
    name: service,
    group: `deploy/${env}`,
    project: PROJECT,
    description: `Build and roll out ${service} to ${env}`,
    href: null,
    permalink: null,
    enabled: true,
    scheduled: false,
    scheduleEnabled: true,
  })),
);

let executionId = 48_210;
const summary = (
  index: number,
  env: string,
  minutesAgo: number,
  status = "succeeded",
) => ({
  execution_id: executionId++,
  status,
  custom_status: null,
  user: USERS[index],
  started_at: iso(minutesAgo),
  ended_at: status === "running" ? null : iso(minutesAgo - 4),
  permalink: null,
  branch: BRANCHES[index],
  options: { BRANCH: BRANCHES[index], ENV: env },
});

const cells = ENVS.flatMap((env, envIndex) =>
  SERVICES.map((service, index) => {
    const running = env === "production" && service === "billing-service";
    const failed = env === "staging" && service === "search-service";
    const latest = summary(
      index,
      env,
      12 + index * 37 + envIndex * 90,
      running ? "running" : failed ? "failed" : "succeeded",
    );
    return {
      service,
      name: service,
      job_id: jobId(service, env),
      group: `deploy/${env}`,
      enabled: true,
      scheduled: false,
      latest,
      deployed: failed
        ? summary(index, env, 600)
        : running
          ? { ...summary(index, env, 1_440), branch: "release/2.13" }
          : latest,
      error: null,
    };
  }),
);

type Cell = (typeof cells)[number];

const runOf = (run: Cell["latest"]) => ({
  executionId: run.execution_id,
  status: run.status,
  statusKind: run.status,
  live: run.status === "running",
  user: run.user,
  startedAt: run.started_at,
  endedAt: run.ended_at,
  branch: run.branch,
});

const sectionsBy = (
  shown: Cell[],
  here: string[],
  key: (below: string[]) => string | null,
) => {
  const by = new Map<string | null, string[]>();
  for (const cell of shown) {
    const segment = key(cell.group.split("/").slice(here.length));
    by.set(segment, [...(by.get(segment) ?? []), cell.job_id]);
  }
  return [...by.entries()]
    .sort(([a], [b]) => (a ?? "").localeCompare(b ?? ""))
    .map(([segment, ids]) => {
      const path = [...here, ...(segment ? [segment] : [])].join("/");
      return {
        segment,
        path,
        label: path || PROJECT,
        prod: path.includes("production"),
        jobs: ids,
      };
    });
};

const jobsView = ({ group }: Params) => {
  const here = typeof group === "string" && group ? group.split("/") : [];
  const shown = cells
    .filter(
      (cell) =>
        cell.group.split("/").slice(0, here.length).join("/") ===
        here.join("/"),
    )
    .sort((a, b) => a.name.localeCompare(b.name));
  const rows = shown.map((cell) => {
    const latest = runOf(cell.latest);
    const deployed = runOf(cell.deployed);
    return {
      jobId: cell.job_id,
      name: cell.name,
      group: cell.group,
      enabled: true,
      scheduled: false,
      error: null,
      latest,
      deployed,
      branch: latest.branch,
      runWord: "Deploy",
      liveBranch:
        latest.live && deployed.branch !== latest.branch
          ? deployed.branch
          : null,
    };
  });
  const compare = sectionsBy(shown, here, (below) => below[0] ?? null).filter(
    (section) => section.segment !== null,
  );
  return {
    project: PROJECT,
    group: here.length ? here.join("/") : null,
    title: here.at(-1) ?? PROJECT,
    error: null,
    partial: false,
    timedOut: 0,
    running: rows.filter((row) => row.latest.live).length,
    failed: rows.filter((row) => row.latest.statusKind === "failed").length,
    jobs: rows,
    list: sectionsBy(shown, here, (below) => below.join("/") || null),
    compare: compare.length > 1 ? compare : [],
    shared: compare.length > 1 ? SERVICES.length : 0,
    loose: 0,
    livePaths: ["", "deploy", "deploy/production"],
  };
};

const LIVE_EXECUTION = 48_199;
const steps = [
  "Checkout",
  "Build image",
  "Push to registry",
  "Migrate database",
  "Roll out",
  "Smoke test",
];

const LOG = [
  [
    "INFO",
    1,
    "Cloning git@github.com:acme/billing-service.git at release/2.14",
  ],
  [
    "INFO",
    1,
    "HEAD is now at 9c41e2a fix(invoices): round VAT per line, not per total",
  ],
  ["INFO", 2, "docker build -t registry.acme.dev/billing-service:9c41e2a ."],
  ["INFO", 2, "#12 [build 6/9] RUN pnpm install --frozen-lockfile"],
  ["INFO", 2, "#12 DONE 18.4s"],
  ["INFO", 2, "#15 exporting to image  \u001b[32mDONE\u001b[0m 3.1s"],
  [
    "INFO",
    3,
    "pushing registry.acme.dev/billing-service:9c41e2a  \u001b[32m✓\u001b[0m 212 MB in 9.8s",
  ],
  ["INFO", 4, "Running migrations: 20260926_add_invoice_line_vat"],
  ["INFO", 4, "\u001b[32m✓\u001b[0m 1 migration applied in 1.2s"],
  ["INFO", 5, "kubectl rollout status deploy/billing-service -n production"],
  [
    "INFO",
    5,
    'Waiting for deployment "billing-service" rollout to finish: 2 of 6 updated replicas are available...',
  ],
  [
    "INFO",
    5,
    'Waiting for deployment "billing-service" rollout to finish: 4 of 6 updated replicas are available...',
  ],
] as const;

const execution = {
  id: LIVE_EXECUTION,
  status: "running",
  customStatus: null,
  user: "edon",
  project: PROJECT,
  "date-started": { date: iso(3), unixtime: Date.now() - 180_000 },
  "date-ended": null,
  permalink: null,
  job: {
    id: jobId("billing-service", "production"),
    name: "billing-service",
    group: "deploy/production",
    project: PROJECT,
    options: { BRANCH: "release/2.14", ENV: "production" },
  },
  argstring: "-BRANCH release/2.14 -ENV production",
};

const workflow = {
  executionState: "RUNNING",
  stepCount: steps.length,
  completed: false,
  steps: steps.map((_step, index) => ({
    id: String(index + 1),
    stepctx: String(index + 1),
    executionState:
      index < 4 ? "SUCCEEDED" : index === 4 ? "RUNNING" : "WAITING",
    startTime: index <= 4 ? iso(3 - index * 0.5) : null,
    endTime: index < 4 ? iso(2.8 - index * 0.5) : null,
    nodeStep: true,
  })),
};

export const RUNDECK_EXECUTION = {
  executionId: LIVE_EXECUTION,
  job: jobs.find((job) => job.id === jobId("billing-service", "production"))!,
};

export const RUNDECK: Record<string, (params: Params) => unknown> = {
  status: () => ({
    configured: true,
    url: "https://rundeck.acme.dev",
    user: "edon",
    token_present: true,
    rundeck_version: "5.8.0",
    ok: true,
    auth_failed: false,
    message: null,
    allow_insecure_private_http: false,
  }),
  projects: () => [
    { name: PROJECT, description: "Customer-facing services" },
    { name: "data", description: "Pipelines and warehouse jobs" },
  ],
  jobIndex: () => [
    { project: PROJECT, jobs, error: null },
    { project: "data", jobs: [], error: null },
  ],
  jobs: () => jobs,
  treeView: () => ({
    projects: [
      {
        name: PROJECT,
        error: null,
        total: jobs.length,
        ungrouped: 0,
        groups: [
          {
            name: "deploy",
            path: "deploy",
            count: jobs.length,
            children: ENVS.map((env) => ({
              name: env,
              path: `deploy/${env}`,
              count: SERVICES.length,
              children: [],
            })),
          },
        ],
      },
      { name: "data", error: null, total: 0, ungrouped: 0, groups: [] },
    ],
  }),
  jobsView,
  jobCells: () => cells,
  jobDetail: ({ jobId: id }) => {
    const job = jobs.find((candidate) => candidate.id === id) ?? jobs[0];
    return {
      ...job,
      execution_enabled: true,
      schedule_enabled: true,
      scheduled: false,
      node_filter: null,
      options: [
        {
          name: "BRANCH",
          label: "Branch",
          description: null,
          required: true,
          secure: false,
          value_exposed: false,
          default: "main",
          values: null,
          values_url: null,
          enforced: false,
          multivalued: false,
          delimiter: null,
          is_date: false,
          date_format: null,
          kind: "text",
        },
      ],
      steps,
    };
  },
  executions: ({ jobId: id }) => history(String(id)),
  jobView: ({ jobId: id }) => jobView(String(id)),
  activityView: () => activityView(),
  deployView: ({ jobId: id, branch, given }) =>
    deployView(
      String(id),
      branch as string | undefined,
      given as Params | undefined,
    ),
  planView: ({ targetBranch }) => ({
    targetBranch,
    deployedBranch: "release/2.13",
    currentBranch: "release/2.14",
    pushes: true,
    checks: [
      {
        tone: "ok",
        text: "Contains everything that is live",
        value: null,
        pushes: false,
      },
      {
        tone: "ok",
        text: "On origin",
        value: `origin/${targetBranch}`,
        pushes: false,
      },
      {
        tone: "info",
        text: "Checkout is on release/2.14",
        value: "9c41e2a",
        pushes: false,
      },
      {
        tone: "warn",
        text: "Checkout has uncommitted changes",
        value: "not deployed",
        pushes: false,
      },
      {
        tone: "warn",
        text: "2 local commits not pushed",
        value: "origin/release/2.14",
        pushes: false,
      },
      {
        tone: "warn",
        text: "Pushes your checkout before deploying",
        value: null,
        pushes: true,
      },
    ],
    detail: null,
  }),
  executionView: () => executionView(),
  deployTargetView: ({ folder }) => {
    const matches = jobs.filter((job) => job.name === folder);
    const place = (job: (typeof jobs)[number]) => ({
      project: job.project,
      jobId: job.id,
      name: job.name,
      group: job.group,
      place: [job.project, ...job.group.split("/"), job.name].join(" / "),
      tone: job.group.includes("production") ? "prod" : "staging",
    });
    const active =
      matches.find((job) => !job.group.includes("production")) ?? matches[0];
    if (!active)
      return {
        jobs: [],
        active: null,
        deployedBranch: null,
        runsBranch: false,
        lastRun: null,
        summary: "",
        readAgainSecs: null,
      };
    const cell = cells.find((candidate) => candidate.job_id === active.id)!;
    const lastRun = `Last run ${cell.latest.status} · ${cell.latest.branch}`;
    return {
      jobs: matches.map(place),
      active: place(active),
      deployedBranch: cell.deployed.branch,
      runsBranch: true,
      lastRun: { statusKind: cell.latest.status, text: lastRun },
      summary: `${place(active).place} runs ${cell.deployed.branch} · ${lastRun}`,
      readAgainSecs: null,
    };
  },
  catalogView: () => ({
    jobs: jobs.map((job) => ({
      project: job.project,
      jobId: job.id,
      name: job.name,
      group: job.group,
      place: [job.project, ...job.group.split("/"), job.name].join(" / "),
      tone: job.group.includes("production") ? "prod" : "staging",
    })),
    failedProjects: 0,
  }),
  checkRun: ({ values }) => ({
    errors: (values as Params).BRANCH
      ? {}
      : { BRANCH: "enter a branch to deploy" },
    send: values,
    valuesUrls: {},
    running: false,
  }),
};

const HISTORY = [
  ["succeeded", 0, 26, "edon"],
  ["succeeded", 1, 1_440, "maya"],
  ["failed", 2, 2_880, "sam"],
  ["succeeded", 1, 2_950, "deploy-bot"],
  ["succeeded", 0, 5_800, "edon"],
  ["succeeded", 1, 8_700, "maya"],
  ["succeeded", 0, 10_100, "deploy-bot"],
  ["failed", 2, 14_400, "sam"],
  ["succeeded", 1, 15_000, "edon"],
  ["succeeded", 0, 20_200, "maya"],
] as const;

/** A job's recent runs, newest first, in the shape Rundeck's execution list returns. */
function history(id: string) {
  const cell = cells.find((candidate) => candidate.job_id === id) ?? cells[0];
  const live = cell.latest?.status === "running";
  const branches = [
    cell.latest?.branch ?? "main",
    cell.deployed?.branch ?? "main",
    "hotfix/vat",
  ];
  return HISTORY.map(([status, branch, minutesAgo, user], index) => {
    const running = live && index === 0;
    const minutes = running ? 3 : minutesAgo;
    const took = 2 + ((index * 7) % 5);
    return {
      id: 48_199 - index * 13,
      status: running ? "running" : status,
      customStatus: null,
      user,
      project: PROJECT,
      "date-started": {
        date: iso(minutes),
        unixtime: Date.now() - minutes * 60_000,
      },
      "date-ended": running
        ? null
        : {
            date: iso(minutes - took),
            unixtime: Date.now() - (minutes - took) * 60_000,
          },
      permalink: null,
      job: {
        id,
        name: cell.name,
        group: cell.group,
        project: PROJECT,
        options: {
          BRANCH: branches[branch],
          ENV: cell.group?.split("/")[1] ?? "",
        },
      },
      argstring: null,
      workflowState: running ? workflow : null,
    };
  });
}

type HistoryRun = ReturnType<typeof history>[number];

const median = (times: number[]) =>
  times.length
    ? [...times].sort((a, b) => a - b)[Math.floor(times.length / 2)]
    : null;

const usualOf = (runs: HistoryRun[]) =>
  median(runs.filter((run) => run.status === "succeeded").map(secsOf));

function secsOf(run: HistoryRun): number {
  return run["date-ended"]
    ? Math.round(
        (run["date-ended"].unixtime - run["date-started"].unixtime) / 1000,
      )
    : 0;
}

function pastRunOf(run: HistoryRun, usual: number | null) {
  return {
    executionId: run.id,
    status: run.status,
    statusKind: run.status,
    live: run.status === "running",
    user: run.user,
    startedAt: run["date-started"].date,
    endedAt: run["date-ended"]?.date ?? null,
    secs: run["date-ended"] ? secsOf(run) : null,
    branch: run.job.options.BRANCH,
    jobId: run.job.id,
    jobName: run.job.name,
    jobGroup: run.job.group,
    usualSecs: usual,
  };
}

function jobView(id: string) {
  const job = jobs.find((candidate) => candidate.id === id) ?? jobs[0];
  const past = history(job.id);
  const usual = usualOf(past);
  const runs = past.map((run) => pastRunOf(run, usual));
  const succeeded = runs.filter((run) => run.status === "succeeded");
  const times = succeeded.map((run) => run.secs ?? 0);
  const live = runs.find((run) => run.live);
  return {
    jobId: job.id,
    name: job.name,
    group: job.group,
    project: PROJECT,
    description: job.description,
    permalink: null,
    executionEnabled: true,
    nodeFilter: null,
    prod: job.group.includes("production"),
    runsBranch: true,
    runWord: "Deploy",
    runs,
    live: live && {
      executionId: live.executionId,
      user: live.user,
      startedAt: live.startedAt,
      branch: live.branch,
      steps: workflow.steps.map((step, index) => ({
        name: steps[index],
        state:
          step.executionState === "SUCCEEDED"
            ? "ok"
            : step.executionState === "RUNNING"
              ? "running"
              : "pending",
      })),
      completed: 4,
      total: steps.length,
    },
    lastGood: succeeded[0] ?? null,
    repeatOptions: succeeded[0]
      ? { BRANCH: succeeded[0].branch, ENV: job.group.split("/")[1] }
      : {},
    finished: runs.filter((run) => !run.live).length,
    succeeded: succeeded.length,
    usualSecs: usual,
    fastestSecs: times.length ? Math.min(...times) : null,
    slowestSecs: times.length ? Math.max(...times) : null,
    readAgainSecs: live ? 3 : null,
  };
}

function deployView(
  id: string,
  branch: string | undefined,
  given: Params | undefined,
) {
  const job = jobs.find((candidate) => candidate.id === id) ?? jobs[0];
  const page = jobView(id);
  const env = job.group.split("/")[1];
  return {
    jobId: job.id,
    name: job.name,
    group: job.group,
    project: PROJECT,
    description: job.description,
    permalink: null,
    nodeFilter: null,
    executionEnabled: true,
    prod: page.prod,
    env: "deploy",
    service: `${job.group}/${job.name}`,
    runsBranch: true,
    branchKey: "BRANCH",
    runWord: "Deploy",
    options: [
      {
        name: "BRANCH",
        label: "Branch",
        description: null,
        required: true,
        secure: false,
        default: "main",
        values: null,
        valuesUrl: null,
        enforced: false,
        multivalued: false,
        delimiter: ",",
        isDate: false,
        dateFormat: null,
        kind: "text",
      },
      {
        name: "ENV",
        label: null,
        description: null,
        required: true,
        secure: false,
        default: env,
        values: ENVS,
        valuesUrl: null,
        enforced: true,
        multivalued: false,
        delimiter: ",",
        isDate: false,
        dateFormat: null,
        kind: "text",
      },
    ],
    optionsError: null,
    values: { BRANCH: branch || String(given?.BRANCH ?? "main"), ENV: env },
    usualSecs: page.usualSecs,
    recentBranches: [...new Set(page.runs.map((run) => run.branch))].slice(
      0,
      6,
    ),
  };
}

function executionView() {
  const done = workflow.steps.filter(
    (step) => step.executionState === "SUCCEEDED",
  ).length;
  return {
    executionId: execution.id,
    status: execution.status,
    statusKind: "running",
    live: true,
    user: execution.user,
    startedAt: execution["date-started"].date,
    endedAt: null,
    permalink: null,
    branch: execution.job.options.BRANCH,
    runWord: "Deploy",
    steps: workflow.steps.map((step, index) => ({
      key: step.stepctx,
      name: steps[index],
      state:
        step.executionState === "SUCCEEDED"
          ? "ok"
          : step.executionState === "RUNNING"
            ? "running"
            : "pending",
      stateName: step.executionState.toLowerCase(),
      startedAt: step.startTime,
      endedAt: step.endTime,
    })),
    completed: done,
    total: steps.length,
    percent: Math.round((done / steps.length) * 100),
    usualSecs: jobView(execution.job.id).usualSecs,
    canRunAgain: true,
    repeatOptions: execution.job.options,
    readAgainSecs: 2,
  };
}

function activityView() {
  const { executions, total } = projectExecutions();
  const usual = new Map(jobs.map((job) => [job.id, usualOf(history(job.id))]));
  const runs = executions.map((run) =>
    pastRunOf(run, usual.get(run.job.id) ?? null),
  );
  return {
    project: PROJECT,
    me: "edon",
    runs,
    total,
    running: runs.filter((run) => run.live).length,
    failed: runs.filter((run) => run.statusKind === "failed").length,
    mine: runs.filter((run) => run.user === "edon").length,
    readAgainSecs: runs.some((run) => run.live) ? 5 : null,
  };
}

function projectExecutions() {
  const executions = jobs
    .flatMap((job, jobIndex) =>
      history(job.id).map((run) =>
        run.status === "running"
          ? run
          : { ...run, id: run.id - (jobIndex + 1) * 1_000 },
      ),
    )
    .sort((a, b) => b["date-started"].unixtime - a["date-started"].unixtime);
  return { executions: executions.slice(0, 50), total: executions.length };
}

export function rundeckStream(
  method: string,
  emit: (value: unknown) => void,
): void {
  if (method === "watch")
    emit({ execution, state: workflow, error: null, terminal: false });
  if (method === "logs") {
    emit({
      entries: LOG.map(([level, step, log], index) => ({
        time: new Date(Date.now() - (LOG.length - index) * 9_000)
          .toISOString()
          .slice(11, 19),
        level,
        log,
        user: "edon",
        stepctx: String(step),
        node: "rundeck-runner-2",
      })),
      offset: String(LOG.length),
      completed: false,
      failed: false,
      error: null,
    });
  }
}
