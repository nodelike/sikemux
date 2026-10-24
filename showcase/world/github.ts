import { parseAnsi } from "../../src/lib/ansi";

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

const pull = (number: number, overrides: Record<string, unknown>) => ({
  ...PULL,
  number,
  url: `https://github.com/nodelike/sikemux/pull/${number}`,
  ...overrides,
});

const PULLS = [
  PULL,
  pull(34, {
    title: "fix(rail): the update chip reads as the one thing to press",
    author: "nodelike",
    authorAssociation: "OWNER",
    head: "fix/update-chip",
    headLabel: "nodelike:fix/update-chip",
    createdAt: iso(95),
    updatedAt: iso(12),
    comments: 0,
    additions: 21,
    deletions: 19,
    changedFiles: 1,
    labels: [{ name: "design", color: "c5def5" }],
    reviewers: [],
  }),
  pull(33, {
    title: "feat(chat): tags in the composer are chips",
    author: "edonramos",
    authorAssociation: "MEMBER",
    head: "feat/composer-chips",
    headLabel: "edonramos:feat/composer-chips",
    draft: true,
    createdAt: iso(1440),
    updatedAt: iso(180),
    comments: 4,
    additions: 512,
    deletions: 120,
    changedFiles: 14,
    labels: [
      { name: "enhancement", color: "a2eeef" },
      { name: "chat", color: "d4c5f9" },
    ],
    reviewers: ["nodelike", "mayachen"],
  }),
  pull(29, {
    title: "perf(terminal): batch PTY output into one frame",
    author: "mayachen",
    authorAssociation: "MEMBER",
    head: "perf/pty-batch",
    headLabel: "mayachen:perf/pty-batch",
    createdAt: iso(4320),
    updatedAt: iso(2600),
    comments: 9,
    additions: 230,
    deletions: 88,
    changedFiles: 6,
    labels: [{ name: "performance", color: "fbca04" }],
    reviewers: ["nodelike"],
  }),
  pull(27, {
    title: "docs: how phones reach a host",
    author: "sam-k",
    authorAssociation: "CONTRIBUTOR",
    head: "docs/remote-access",
    headLabel: "sam-k:docs/remote-access",
    createdAt: iso(7200),
    updatedAt: iso(5000),
    comments: 2,
    additions: 64,
    deletions: 3,
    changedFiles: 2,
    labels: [{ name: "docs", color: "0075ca" }],
    reviewers: [],
  }),
];

const CLOSED_PULLS = [
  pull(30, {
    title: "feat(git): review a commit before pushing it",
    author: "nodelike",
    state: "closed",
    mergedAt: iso(1500),
    mergedBy: "nodelike",
    updatedAt: iso(1500),
    labels: [{ name: "enhancement", color: "a2eeef" }],
  }),
  pull(28, {
    title: "chore: try a second rail for agents",
    author: "edonramos",
    state: "closed",
    updatedAt: iso(6000),
    labels: [],
  }),
  pull(26, {
    title: "fix(browser): keep a parked tab awake while it loads",
    author: "mayachen",
    state: "closed",
    mergedAt: iso(9000),
    mergedBy: "nodelike",
    updatedAt: iso(9000),
    labels: [{ name: "bug", color: "d73a4a" }],
  }),
];

const issue = (
  number: number,
  title: string,
  minutes: number,
  overrides: Record<string, unknown> = {},
) => ({
  number,
  title,
  body: "Steps to reproduce:\n\n1. Open a chat with an agent.\n2. Switch the pane to its terminal and back.\n\nWhat happens: the draft is gone.\nWhat should happen: the draft is still there.",
  state: "open",
  stateReason: null,
  author: "edonramos",
  avatarUrl: null,
  createdAt: iso(minutes),
  updatedAt: iso(Math.max(5, minutes / 3)),
  closedAt: null,
  comments: 0,
  labels: [],
  assignees: [],
  url: `https://github.com/nodelike/sikemux/issues/${number}`,
  ...overrides,
});

const ISSUES = [
  issue(35, "Agents: a draft survives switching between GUI and TUI", 40, {
    comments: 3,
    labels: [
      { name: "bug", color: "d73a4a" },
      { name: "chat", color: "d4c5f9" },
    ],
    assignees: ["nodelike"],
  }),
  issue(
    32,
    "Composer: tag issues, pull requests, agents and files inline",
    300,
    {
      author: "nodelike",
      comments: 6,
      labels: [
        { name: "enhancement", color: "a2eeef" },
        { name: "design", color: "c5def5" },
      ],
      assignees: ["nodelike", "edonramos"],
    },
  ),
  issue(25, "Epic: desktop UI on GPUI for speed and memory", 4300, {
    author: "nodelike",
    comments: 14,
    labels: [{ name: "epic", color: "3e4b9e" }],
  }),
  issue(
    24,
    "Phone: hold sends while disconnected and say when the phone is the one offline",
    5200,
    {
      author: "mayachen",
      comments: 2,
      labels: [{ name: "mobile", color: "bfdadc" }],
      assignees: ["mayachen"],
    },
  ),
  issue(
    22,
    "Agents keep working through Wi-Fi drops and network changes",
    8000,
    {
      comments: 8,
      labels: [
        { name: "epic", color: "3e4b9e" },
        { name: "reliability", color: "e99695" },
      ],
    },
  ),
  issue(19, "Documentation site at docs.sikemux.com", 12000, {
    author: "sam-k",
    comments: 1,
    labels: [{ name: "docs", color: "0075ca" }],
  }),
  issue(
    17,
    "Welcome screen: offer install and sign in for every agent",
    15000,
    {
      comments: 5,
      labels: [{ name: "onboarding", color: "f9d0c4" }],
      assignees: ["edonramos"],
    },
  ),
];

const CLOSED_ISSUES = [
  issue(21, "Rail rows sit 2px right of the project rows", 7000, {
    state: "closed",
    stateReason: "completed",
    closedAt: iso(6800),
    labels: [{ name: "bug", color: "d73a4a" }],
  }),
  issue(16, "Two rails for agents and projects", 20000, {
    state: "closed",
    stateReason: "not_planned",
    closedAt: iso(15000),
    labels: [{ name: "design", color: "c5def5" }],
  }),
];

const release = (
  id: number,
  tag: string,
  minutes: number,
  overrides: Record<string, unknown> = {},
) => ({
  id,
  tag,
  name: tag,
  body: "## What's new\n\n- Tags in the composer are chips you can delete in one go.\n- The update chip reads as the thing to press.\n\n## Fixes\n\n- A draft survives switching an agent between chat and terminal.",
  draft: false,
  prerelease: tag.includes("nightly"),
  publishedAt: iso(minutes),
  author: "github-actions",
  assets: [
    {
      id: id * 10 + 1,
      name: `Sikemux_${tag.replace(/^v/, "")}_aarch64.dmg`,
      sizeBytes: 16_400_000,
      downloads: tag.includes("nightly") ? 41 : 1280,
    },
    {
      id: id * 10 + 2,
      name: "latest.json",
      sizeBytes: 1_900,
      downloads: tag.includes("nightly") ? 310 : 9800,
    },
  ],
  url: `https://github.com/nodelike/sikemux/releases/tag/${tag}`,
  ...overrides,
});

const RELEASES = [
  release(9, "v0.5.3-nightly.6", 330),
  release(8, "v0.5.3-nightly.5", 1800),
  release(7, "v0.5.2", 5800),
  release(6, "v0.5.1", 14000),
  release(5, "v0.5.0", 26000),
];

const note = (
  id: number,
  title: string,
  kind: string,
  reason: string,
  number: number | null,
  minutes: number,
  unread: boolean,
) => ({
  id: String(id),
  title,
  kind,
  reason,
  repo: "nodelike/sikemux",
  number,
  unread,
  updatedAt: iso(minutes),
  url: number
    ? `https://github.com/nodelike/sikemux/${kind === "Issue" ? "issues" : "pull"}/${number}`
    : null,
});

const INBOX = [
  note(
    1,
    "feat(github): the run page, pull request reviews and checks",
    "PullRequest",
    "review_requested",
    31,
    25,
    true,
  ),
  note(
    2,
    "Agents: a draft survives switching between GUI and TUI",
    "Issue",
    "assign",
    35,
    40,
    true,
  ),
  note(
    3,
    "feat(chat): tags in the composer are chips",
    "PullRequest",
    "mention",
    33,
    180,
    true,
  ),
  note(
    4,
    "Release v0.5.3-nightly.6",
    "CheckSuite",
    "ci_activity",
    null,
    330,
    false,
  ),
  note(
    5,
    "perf(terminal): batch PTY output into one frame",
    "PullRequest",
    "comment",
    29,
    2600,
    false,
  ),
  note(
    6,
    "Epic: desktop UI on GPUI for speed and memory",
    "Issue",
    "subscribed",
    25,
    4300,
    false,
  ),
];

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
  outcome: "success",
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
    outcome: "failure",
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
    outcome: "running",
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
  after = 0,
) => ({
  number,
  name,
  status: "completed",
  conclusion,
  outcome: conclusion,
  startedAt: at(from, after),
  completedAt: at(from, after + seconds),
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
  outcome: conclusion,
  startedAt: at(from),
  completedAt: at(from, minutes * 60),
  runner: "GitHub Actions 12",
  url: `https://github.com/nodelike/sikemux/actions/runs/36316473434/job/${id}`,
  checkRunId: id + 1,
  steps,
});

const RELEASE_JOBS = [
  job(108612011312, "Checks / What changed", 361, 0.4),
  job(
    108612011313,
    "Checks / Frontend quality, build, and tests",
    360,
    3.1,
    "success",
    [
      step(1, "Set up job", 360, 2),
      step(2, "Check out", 360, 4, "success", 2),
      step(3, "Install dependencies", 360, 38, "success", 6),
      step(4, "Lint and typecheck", 359, 52),
      step(5, "Run tests", 358, 70),
      step(6, "Complete job", 357, 1),
    ],
  ),
  job(108612011314, "Checks / Rust formatting, lint and tests", 360, 2.95),
  job(108612011315, "Checks / macOS launched smoke test", 360, 6.6),
  job(108612011316, "Build, verify, and publish", 353, 20.3),
  job(108612011317, "Build the Linux host core (x86_64)", 353, 8.4),
  job(108612011318, "Build the Linux host core (aarch64)", 353, 9.1),
];

const CI_JOBS = [
  job(208612011001, "Frontend", 42, 3.2, "success"),
  job(208612011002, "Rust", 42, 6.1, "failure", [
    step(1, "Set up job", 42, 2),
    step(2, "Check out", 42, 3, "success", 2),
    step(3, "Install dependencies", 42, 40, "success", 5),
    step(4, "cargo fmt --check", 42, 8, "success", 45),
    step(5, "cargo clippy", 42, 150, "failure", 53),
    step(6, "cargo test", 42, 0, "skipped", 203),
    step(7, "Complete job", 42, 1, "success", 203),
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

const WORKFLOW_FILES: Record<string, string> = {
  [`${SHA}:.github/workflows/release.yml`]: `name: Release
on:
  push:
    tags: ["v*"]
jobs:
  checks:
    name: Checks
    uses: ./.github/workflows/ci.yml
  build:
    name: Build, verify, and publish
    needs: checks
    runs-on: macos-15
  build-host:
    name: Build the Linux host core (\${{ matrix.arch }})
    needs: [checks]
    strategy:
      matrix:
        arch: [x86_64, aarch64]
    runs-on: ubuntu-24.04
`,
  [`${SHA}:.github/workflows/ci.yml`]: `name: CI
on:
  workflow_call:
jobs:
  changes:
    name: What changed
    runs-on: ubuntu-latest
  frontend:
    name: Frontend quality, build, and tests
    needs: changes
    runs-on: macos-15
  rust:
    name: Rust formatting, lint and tests
    needs: changes
    runs-on: macos-15
  macos:
    name: macOS launched smoke test
    needs:
      - changes
    runs-on: macos-15
`,
  [`${PR_SHA}:.github/workflows/ci.yml`]: `name: CI
on: pull_request
jobs:
  frontend:
    name: Frontend
    runs-on: macos-15
  rust:
    name: Rust
    runs-on: macos-15
  package:
    name: Package
    needs: [frontend, rust]
    runs-on: macos-15
`,
};

function stepLog(name: string, conclusion: string): string[] {
  if (name === "Set up job")
    return [
      "Current runner version: '2.337.0'",
      "##[group]Runner Image",
      "Image: macos-15-arm64",
      "Version: 20261012.601",
      "##[endgroup]",
      "Prepare workflow directory",
      "Download action repository 'actions/checkout@v5'",
    ];
  if (name === "Check out")
    return [
      "##[group]Run actions/checkout@v5",
      "Syncing repository: nodelike/sikemux",
      "##[endgroup]",
      "Fetching the repository",
      "HEAD is now at 69ed244 fix(github): a job's own summary",
    ];
  if (name === "Install dependencies")
    return [
      "##[group]Run pnpm install --frozen-lockfile",
      "\u001b[36;1mpnpm install --frozen-lockfile\u001b[0m",
      "shell: /bin/bash -e {0}",
      "##[endgroup]",
      ...LOG_LINES.slice(7, 176),
    ];
  if (name === "cargo clippy")
    return [
      "##[group]Run flags=(--workspace)",
      "\u001b[36;1mflags=(--workspace)\u001b[0m",
      '\u001b[36;1m[[ "$CHANNEL" == "nightly" ]] && flags+=(--all-targets)\u001b[0m',
      '\u001b[36;1mcargo clippy "${flags[@]}" -- -D warnings\u001b[0m',
      "shell: /usr/bin/bash -e {0}",
      "env:",
      "  CARGO_TERM_COLOR: always",
      "  CHANNEL: nightly",
      "##[endgroup]",
      "\u001b[1m\u001b[92m    Checking\u001b[0m sikemux-plugin-github v0.5.3 (src-tauri/plugins/github)",
      "\u001b[0m\u001b[1m\u001b[38;5;9merror\u001b[0m\u001b[0m\u001b[1m: this expression creates a reference which is immediately dereferenced by the compiler\u001b[0m",
      "\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m--> \u001b[0m\u001b[0msrc-tauri/plugins/github/src/runs.rs:412:31\u001b[0m",
      "\u001b[0m    \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m",
      "\u001b[0m\u001b[1m\u001b[38;5;12m412\u001b[0m\u001b[0m \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\u001b[0m \u001b[0m\u001b[0m    let page = client::get(&data_dir, &path).await?;\u001b[0m",
      "\u001b[0m    \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\u001b[0m                            \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;9m^^^^^^^^^\u001b[0m\u001b[0m \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;9mhelp: change this to: `data_dir`\u001b[0m",
      "\u001b[0m    \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: `-D clippy::needless-borrow` implied by `-D warnings`\u001b[0m",
      "\u001b[1m\u001b[91merror\u001b[0m\u001b[1m:\u001b[0m could not compile `sikemux-plugin-github` (lib) due to 1 previous error",
      conclusion === "failure"
        ? "##[error]Process completed with exit code 101."
        : "Finished",
    ];
  if (name === "Complete job")
    return ["Post job cleanup.", "Cleaning up orphan processes"];
  return [`##[group]Run ${name}`, name, "##[endgroup]", "Done"];
}

function jobsFor(runId: number) {
  return runId === 36316470001 ? CI_JOBS : RELEASE_JOBS;
}

const box = (id: string, title: string | null, jobs: [number, string][]) => ({
  id,
  title,
  jobs: jobs.map(([job, label]) => ({
    job: String(job),
    label,
    skipped: false,
  })),
});

const edges = (...pairs: string[]) =>
  pairs.map((pair) => {
    const [from, to] = pair.split(">");
    return { from, to };
  });

/** Each run's jobs as the core lays them out from its workflow files. */
const RUN_GRAPHS: Record<number, unknown> = {
  36316473434: {
    columns: [
      [box("checks/changes", null, [[108612011312, "Checks / What changed"]])],
      [
        box("checks/frontend", null, [
          [108612011313, "Checks / Frontend quality, build, and tests"],
          [108612011314, "Checks / Rust formatting, lint and tests"],
          [108612011315, "Checks / macOS launched smoke test"],
        ]),
      ],
      [
        box("build", null, [[108612011316, "Build, verify, and publish"]]),
        box("build-host", "Build the Linux host core", [
          [108612011317, "x86_64"],
          [108612011318, "aarch64"],
        ]),
      ],
    ],
    edges: edges(
      "checks/changes>checks/frontend",
      "checks/frontend>build",
      "checks/frontend>build-host",
    ),
  },
  36316470001: {
    columns: [
      [
        box("frontend", null, [
          [208612011001, "Frontend"],
          [208612011002, "Rust"],
        ]),
      ],
      [box("package", null, [[208612011003, "Package"]])],
    ],
    edges: edges("frontend>package"),
  },
};

/** A run as the core opens it. */
export function openRun(runId: number) {
  const { run, jobs } = GITHUB.run({ runId }) as {
    run: { id: number; status: string };
    jobs: ReturnType<typeof job>[];
  };
  const unfinished = run.status !== "completed";
  const failed = jobs.filter((each) => each.outcome === "failure");
  return {
    run,
    jobs: jobs.map((each) => ({
      job: { ...each, id: String(each.id) },
      stoppedAt:
        (each.steps as ReturnType<typeof step>[]).find(
          (one) => one.outcome === "failure",
        )?.name ?? null,
    })),
    attempts: 1,
    unfinished,
    done: jobs.filter((each) => each.status === "completed").length,
    failed: failed.length,
    failedJob: failed[0] ? String(failed[0].id) : null,
    graph: RUN_GRAPHS[run.id] ?? {
      columns: [
        jobs.map((each) => box(`job:${each.id}`, null, [[each.id, each.name]])),
      ],
      edges: [],
    },
    timing: unfinished ? null : GITHUB.runTiming({}),
    summaries: unfinished
      ? []
      : jobs
          .filter((each) => each.conclusion !== "skipped")
          .map((each) => String(each.id)),
  };
}

const MARKUP =
  /^##\[(group|endgroup|section|command|error|warning|notice|debug)\]/;

const colour = (color: number | string | null) =>
  color === null
    ? undefined
    : typeof color === "number"
      ? { kind: "palette", index: color }
      : { kind: "rgb", hex: color };

/** A job's log as the core splits it into its steps. */
export function openJobLog(jobId: string) {
  const found = [...RELEASE_JOBS, ...CI_JOBS].find(
    (each) => String(each.id) === jobId,
  );
  const steps = (found?.steps ?? []) as ReturnType<typeof step>[];
  const texts: string[] = steps.length === 0 ? [...LOG_LINES] : [];
  const sections =
    steps.length === 0
      ? [{ step: null, from: 0, to: texts.length }]
      : steps.map((each) => {
          const from = texts.length;
          if (each.conclusion !== "skipped")
            texts.push(...stepLog(each.name, each.conclusion));
          return { step: each.number, from, to: texts.length };
        });
  const lines = texts.map((raw, index) => {
    const mark = MARKUP.exec(raw)?.[1] ?? null;
    const runs = parseAnsi(raw.replace(MARKUP, ""));
    const styled = runs.some(
      (run) => run.fg !== null || run.bg !== null || run.bold || run.dim,
    );
    return {
      number: index + 1,
      text: runs.map((run) => run.text).join(""),
      spans: styled
        ? runs.map((run) => ({
            text: run.text,
            fg: colour(run.fg),
            bg: colour(run.bg),
            bold: run.bold,
            dim: run.dim,
            italic: run.italic,
            underline: run.underline,
          }))
        : [],
      mark,
      script: false,
    };
  });
  for (const { from, to } of sections) {
    let running = false;
    for (const line of lines.slice(from, to)) {
      if (line.mark === "group" && line.text.startsWith("Run ")) running = true;
      else if (
        running &&
        (line.mark === "endgroup" || line.text.startsWith("shell: "))
      )
        running = false;
      else if (running) line.script = true;
    }
  }
  return {
    lines,
    from: 0,
    total: lines.length,
    sections,
    expired: false,
    truncated: false,
  };
}

const PULL_TIMELINE = [
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
];

const ISSUE_TIMELINE = [
  {
    kind: "assigned",
    id: 21,
    actor: "edonramos",
    avatarUrl: null,
    association: null,
    at: iso(38),
    body: null,
    state: null,
    sha: null,
    message: null,
    subject: "nodelike",
  },
  {
    kind: "labeled",
    id: 22,
    actor: "nodelike",
    avatarUrl: null,
    association: null,
    at: iso(30),
    body: null,
    state: null,
    sha: null,
    message: null,
    subject: "bug",
  },
  {
    kind: "cross-referenced",
    id: null,
    actor: "nodelike",
    avatarUrl: null,
    association: null,
    at: iso(25),
    body: null,
    state: null,
    sha: null,
    message: null,
    subject: "fix(chat): a draft survives switching an agent between its chat and its TUI",
    source: { number: 136, pull: true, state: "open", merged: false, repo: "nodelike/sikemux" },
  },
  {
    kind: "commented",
    id: 23,
    actor: "nodelike",
    avatarUrl: null,
    association: "OWNER",
    at: iso(20),
    body: "Reproduced. The draft lives in the chat view, so the terminal view never sees it.",
    state: null,
    sha: null,
    message: null,
    subject: null,
  },
  {
    kind: "commented",
    id: 24,
    actor: "mayachen",
    avatarUrl: null,
    association: "MEMBER",
    at: iso(8),
    body: "Same on the phone when it reconnects.",
    state: null,
    sha: null,
    message: null,
    subject: null,
  },
];

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
    const steps = (found?.steps ?? []) as ReturnType<typeof step>[];
    if (steps.length === 0)
      return {
        expired: false,
        truncated: false,
        lines: LOG_LINES.map((text, index) => ({
          number: index + 1,
          timestamp: new Date(start + index * 800 + 500).toISOString(),
          text,
        })),
      };
    const lines = steps
      .filter((each) => each.conclusion !== "skipped")
      .flatMap((each) =>
        stepLog(each.name, each.conclusion).map((text, index) => ({
          text,
          timestamp: new Date(
            Date.parse(each.startedAt) + 300 + index * 20,
          ).toISOString(),
        })),
      );
    return {
      expired: false,
      truncated: false,
      lines: lines.map((each, index) => ({ number: index + 1, ...each })),
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
          {
            path: ".github/workflows/ci.yml",
            startLine: 61,
            endLine: 61,
            level: "warning",
            title: null,
            message:
              "Node.js 20 actions are deprecated. Update actions/cache to a version that runs on Node.js 24.",
            details: null,
          },
          {
            path: null,
            startLine: null,
            endLine: null,
            level: "notice",
            title: null,
            message:
              "Due to capacity constraints, jobs targeting macOS arm64 runners may experience longer queue times.",
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
  workflowFileAt: ({ path, gitRef }) => {
    const text = WORKFLOW_FILES[`${String(gitRef)}:${String(path)}`];
    if (!text) throw { category: "unavailable", message: "No such file" };
    return { path, text };
  },
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
  issueCounts: () => ({ open: 4, closed: 12 }),
  pullCounts: () => ({ open: PULLS.length, closed: CLOSED_PULLS.length }),
  pulls: ({ state }) =>
    state === "closed"
      ? CLOSED_PULLS
      : state === "all"
        ? [...PULLS, ...CLOSED_PULLS]
        : PULLS,
  pull: ({ number }) =>
    [...PULLS, ...CLOSED_PULLS].find((item) => item.number === number) ?? PULL,
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
  timeline: ({ number }) =>
    [...ISSUES, ...CLOSED_ISSUES].some((item) => item.number === number)
      ? ISSUE_TIMELINE
      : PULL_TIMELINE,
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
  issues: ({ state }) => {
    const issues =
      state === "closed"
        ? CLOSED_ISSUES
        : state === "all"
          ? [...ISSUES, ...CLOSED_ISSUES]
          : ISSUES;
    return { issues, total: issues.length, nextPage: null };
  },
  issue: ({ number }) =>
    [...ISSUES, ...CLOSED_ISSUES].find((item) => item.number === number) ??
    ISSUES[0],
  releases: () => RELEASES,
  inbox: ({ all }) => (all ? INBOX : INBOX.filter((item) => item.unread)),
};
