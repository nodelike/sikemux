type Params = Record<string, unknown>;

const iso = (minutesAgo: number) =>
  new Date(Date.now() - minutesAgo * 60_000).toISOString();
const at = (minutesAgo: number, seconds = 0) =>
  new Date(Date.now() - minutesAgo * 60_000 + seconds * 1000).toISOString();

const REPO = { host: "github.com", owner: "nodelike", name: "sikemux" };
const SHA = "5b6f3a8c1d2e4f60718293a4b5c6d7e8f9012345";
const PR_SHA = "69ed2441741d57fcf30037bf930983f504de506d";

const PULL = {
  number: 31,
  title: "feat(github): the run page, pull request reviews and checks",
  body: "Brings the run page up to what GitHub shows, and lets a pull request be reviewed without leaving Sikemux.\n\n- A job graph\n- Checks on the pull request\n- Approve or request changes",
  state: "open",
  draft: false,
  author: "Sujalxcode",
  avatarUrl: null,
  authorAssociation: "COLLABORATOR",
  head: "feat/github-actions-plugin",
  headLabel: "Sujalxcode:feat/github-actions-plugin",
  base: "main",
  headSha: PR_SHA,
  createdAt: iso(300),
  updatedAt: iso(33),
  comments: 1,
  additions: 1840,
  deletions: 96,
  changedFiles: 3,
  mergeable: true,
  mergeState: "clean",
  labels: [{ name: "enhancement", color: "a2eeef" }],
  reviewers: ["nodelike"],
  assignees: [],
  milestone: null,
  commits: 3,
  mergedAt: null,
  mergedBy: null,
  mergeCommitSha: null,
  avatars: {},
  url: "https://github.com/nodelike/sikemux/pull/31",
};

const run = (id: number, overrides: Record<string, unknown>) => ({
  id,
  name: "Release",
  title: "Release v0.4.2-nightly.3",
  workflowId: 11,
  path: ".github/workflows/release.yml",
  runNumber: 7,
  attempt: 1,
  event: "push",
  status: "completed",
  conclusion: "success",
  branch: "v0.4.2-nightly.3",
  sha: SHA,
  shortSha: SHA.slice(0, 7),
  actor: "nodelike",
  avatarUrl: null,
  createdAt: iso(360),
  startedAt: iso(360),
  updatedAt: iso(326),
  pullRequests: [],
  url: `https://github.com/nodelike/sikemux/actions/runs/${id}`,
  ...overrides,
});

const RUNS = [
  run(36316473434, {}),
  run(36316470001, {
    name: "CI",
    title: "fix(github): a job's own summary",
    workflowId: 12,
    path: ".github/workflows/ci.yml",
    runNumber: 412,
    event: "pull_request",
    branch: "feat/github-actions-plugin",
    conclusion: "failure",
    createdAt: iso(42),
    startedAt: iso(42),
    updatedAt: iso(33),
    pullRequests: [31],
    actor: "Sujalxcode",
    sha: PR_SHA,
    shortSha: PR_SHA.slice(0, 7),
  }),
  run(36316470002, {
    name: "CI",
    title: "perf(github): redraw a row only when it changed",
    workflowId: 12,
    path: ".github/workflows/ci.yml",
    runNumber: 413,
    event: "push",
    branch: "main",
    status: "in_progress",
    conclusion: null,
    createdAt: iso(4),
    startedAt: iso(4),
    updatedAt: iso(1),
    actor: "Sujalxcode",
  }),
];

const step = (
  number: number,
  name: string,
  from: number,
  seconds: number,
  conclusion = "success",
) => ({
  number,
  name,
  status: "completed",
  conclusion,
  startedAt: at(from),
  completedAt: at(from, seconds),
});

const job = (
  id: number,
  name: string,
  from: number,
  minutes: number,
  conclusion = "success",
  steps: unknown[] = [],
) => ({
  id,
  name,
  status: "completed",
  conclusion,
  startedAt: at(from),
  completedAt: at(from, minutes * 60),
  runner: "GitHub Actions 12",
  url: `https://github.com/nodelike/sikemux/actions/runs/36316473434/job/${id}`,
  checkRunId: id + 1,
  steps,
});

const RELEASE_JOBS = [
  job(
    108612011313,
    "Checks / Frontend quality, build, and tests",
    360,
    3.1,
    "success",
    [
      step(1, "Set up job", 360, 2),
      step(2, "Check out", 360, 4),
      step(3, "Install dependencies", 360, 38),
      step(4, "Lint and typecheck", 359, 52),
      step(5, "Run tests", 358, 70),
      step(6, "Complete job", 357, 1),
    ],
  ),
  job(108612011314, "Checks / Rust formatting, lint and tests", 360, 2.95),
  job(108612011315, "Checks / macOS launched smoke test", 360, 6.6),
  job(108612011316, "Build, verify, and publish", 353, 20.3),
];

const CI_JOBS = [
  job(208612011001, "Frontend", 42, 3.2, "success"),
  job(208612011002, "Rust", 42, 6.1, "failure", [
    step(1, "Set up job", 42, 2),
    step(2, "Check out", 42, 3),
    step(3, "cargo clippy", 42, 150, "failure"),
    step(4, "cargo test", 39, 0, "skipped"),
  ]),
  job(208612011003, "Package", 35, 0, "skipped"),
];

const LOG_LINES = [
  "##[group]Runner Image",
  "Image: macos-15-arm64",
  "##[endgroup]",
  "##[group]Run actions/checkout@v5",
  "Syncing repository: nodelike/sikemux",
  "##[endgroup]",
  "##[group]Run pnpm install --frozen-lockfile",
  "Lockfile is up to date, resolution step is skipped",
  ...Array.from(
    { length: 160 },
    (_, index) =>
      `Progress: resolved ${(index + 1) * 5}, reused ${(index + 1) * 5}, downloaded 0, added ${index * 5}`,
  ),
  "Packages: +812",
  "Done in 31.2s",
  "##[endgroup]",
  "##[group]Run pnpm lint && pnpm typecheck",
  "> eslint --max-warnings=0 src/**/*.{ts,tsx}",
  "> tsc --noEmit",
  "##[endgroup]",
  "##[group]Run pnpm test",
  " ✓ src/plugins/github/jobGraph.test.ts (8 tests) 4ms",
  " ✓ src/plugins/github/runStatus.test.ts (14 tests) 6ms",
  " Test Files  77 passed (77)",
  "      Tests  433 passed (433)",
  "##[endgroup]",
  "Cleaning up orphan processes",
];

function jobsFor(runId: number) {
  return runId === 36316470001 ? CI_JOBS : RELEASE_JOBS;
}

export const GITHUB: Record<string, (params: Params) => unknown> = {
  status: () => ({
    configured: true,
    account: "github.com:Sujalxcode",
    host: "github.com",
    login: "Sujalxcode",
    tokenSource: "ghCli",
    tokenVariable: null,
    scopes: ["repo", "workflow"],
    canWriteWorkflows: true,
    ok: true,
    authFailed: false,
    message: null,
  }),
  accounts: () => [
    {
      id: "github.com:Sujalxcode",
      host: "github.com",
      login: "Sujalxcode",
      source: "ghCli",
      isDefault: true,
    },
  ],
  accountFor: () => "github.com:Sujalxcode",
  rateLimit: () => ({
    limited: false,
    resetsAt: null,
    remaining: 4870,
    limit: 5000,
    near: false,
  }),
  resolveRemote: () => ({
    repo: REPO,
    slug: "nodelike/sikemux",
    sameHost: true,
  }),
  myRepos: () => [
    {
      owner: "nodelike",
      name: "sikemux",
      slug: "nodelike/sikemux",
      private: false,
      archived: false,
      defaultBranch: "main",
      pushedAt: iso(4),
      url: "https://github.com/nodelike/sikemux",
    },
  ],
  workflows: () => [
    {
      id: 11,
      name: "Release",
      path: ".github/workflows/release.yml",
      state: "active",
      active: true,
      url: "",
    },
    {
      id: 12,
      name: "CI",
      path: ".github/workflows/ci.yml",
      state: "active",
      active: true,
      url: "",
    },
  ],
  runs: ({ headSha }) => {
    const runs = headSha ? RUNS.filter((each) => each.sha === headSha) : RUNS;
    return { runs, total: runs.length, nextPage: null };
  },
  run: ({ runId }) => ({
    run: RUNS.find((each) => each.id === runId) ?? RUNS[0],
    jobs: jobsFor(runId as number),
  }),
  runAttempt: ({ runId }) => ({
    run: RUNS.find((each) => each.id === runId) ?? RUNS[0],
    jobs: jobsFor(runId as number),
  }),
  jobLog: ({ jobId }) => {
    const found = [...RELEASE_JOBS, ...CI_JOBS].find(
      (each) => each.id === jobId,
    );
    const start = Date.parse(found?.startedAt ?? iso(10));
    return {
      expired: false,
      truncated: false,
      lines: LOG_LINES.map((text, index) => ({
        number: index + 1,
        timestamp: new Date(start + index * 800 + 500).toISOString(),
        text,
      })),
    };
  },
  annotations: ({ checkRunId }) =>
    checkRunId === 208612011003
      ? [
          {
            path: "src-tauri/plugins/github/src/runs.rs",
            startLine: 412,
            endLine: 412,
            level: "failure",
            title: "clippy::needless_borrow",
            message:
              "this expression creates a reference which is immediately dereferenced",
            details: null,
          },
        ]
      : [],
  jobSummary: ({ checkRunId }) =>
    checkRunId === 108612011314
      ? {
          title: "Vitest Test Report",
          body: "## Vitest Test Report\n\n| Files | Tests | Passed | Failed |\n| --- | --- | --- | --- |\n| 77 | 433 | 433 | 0 |\n",
        }
      : null,
  runTiming: () => ({ runDurationMs: 34 * 60_000 + 27_000, billable: [] }),
  workflowFile: () => ({
    path: ".github/workflows/release.yml",
    text: "name: Release\non:\n  push:\n    tags: ['v*']\njobs:\n  checks:\n    uses: ./.github/workflows/checks.yml\n  publish:\n    name: Build, verify, and publish\n    needs: checks\n    runs-on: macos-15\n",
  }),
  artifacts: () => [
    {
      id: 1,
      name: "Sikemux_0.4.2-nightly.3_aarch64.dmg",
      sizeBytes: 10_400_000,
      expired: false,
      createdAt: iso(327),
      expiresAt: null,
    },
  ],
  pendingApprovals: () => [],
  pulls: ({ state }) => (state === "closed" ? [] : [PULL]),
  pull: () => PULL,
  pullFiles: () => [
    {
      path: "src/plugins/github/jobGraph.ts",
      status: "added",
      additions: 3,
      deletions: 0,
      previousPath: null,
      patch:
        "@@ -0,0 +1,3 @@\n+export function stagesOf() {\n+    return [];\n+}",
    },
  ],
  pullCommits: () => [
    {
      sha: "c1f2023a8d1b",
      message: "feat(github): a job graph for a run",
      author: "Sujalxcode",
      avatarUrl: null,
      date: iso(290),
    },
    {
      sha: "775060ae44c2",
      message: "feat(github): checks on a pull request",
      author: "Sujalxcode",
      avatarUrl: null,
      date: iso(240),
    },
    {
      sha: "bff33fa90b17",
      message: "feat(github): approve or request changes",
      author: "Sujalxcode",
      avatarUrl: null,
      date: iso(200),
    },
  ],
  timeline: () => [
    ...[
      ["c1f2023a8d1b", "feat(github): a job graph for a run", 290],
      ["775060ae44c2", "feat(github): checks on a pull request", 240],
      ["bff33fa90b17", "feat(github): approve or request changes", 200],
    ].map(([sha, message, minutes]) => ({
      kind: "committed",
      id: null,
      actor: "Sujalxcode",
      avatarUrl: null,
      association: null,
      at: iso(minutes as number),
      body: null,
      state: null,
      sha,
      message,
      subject: null,
    })),
    {
      kind: "review_requested",
      id: 11,
      actor: "Sujalxcode",
      avatarUrl: null,
      association: null,
      at: iso(190),
      body: null,
      state: null,
      sha: null,
      message: null,
      subject: "nodelike",
    },
    {
      kind: "labeled",
      id: 12,
      actor: "nodelike",
      avatarUrl: null,
      association: null,
      at: iso(150),
      body: null,
      state: null,
      sha: null,
      message: null,
      subject: "enhancement",
    },
    {
      kind: "reviewed",
      id: 13,
      actor: "nodelike",
      avatarUrl: null,
      association: "OWNER",
      at: iso(120),
      body: "The graph should keep a called workflow's jobs in one box.",
      state: "changes_requested",
      sha: null,
      message: null,
      subject: null,
    },
    {
      kind: "commented",
      id: 14,
      actor: "nodelike",
      avatarUrl: null,
      association: "OWNER",
      at: iso(90),
      body: "Nice, this saves a trip to GitHub.",
      state: null,
      sha: null,
      message: null,
      subject: null,
    },
  ],
  pullReviews: () => [
    {
      author: "nodelike",
      avatarUrl: null,
      state: "CHANGES_REQUESTED",
      body: "The graph should keep a called workflow's jobs in one box.",
      submittedAt: iso(120),
    },
  ],
  comments: () => [
    {
      id: 1,
      author: "nodelike",
      avatarUrl: null,
      authorAssociation: "OWNER",
      body: "Nice, this saves a trip to GitHub.",
      createdAt: iso(90),
      url: null,
    },
  ],
  branches: () => ["main", "feat/github-actions-plugin", "release/0.4"],
  issues: () => [],
  releases: () => [],
  inbox: () => [],
};
