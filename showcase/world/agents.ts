import { AGENTS } from "./workspace";
import { FRONT, SIKEMUX } from "./projects";

type Update = Record<string, unknown>;

let toolId = 0;
let messageId = 0;

const user = (text: string): Update => ({
  sessionUpdate: "user_message_chunk",
  messageId: `m${++messageId}`,
  content: { type: "text", text },
});
const say = (text: string): Update => ({
  sessionUpdate: "agent_message_chunk",
  messageId: `m${++messageId}`,
  content: { type: "text", text },
});
const think = (text: string): Update => ({
  sessionUpdate: "agent_thought_chunk",
  messageId: `m${++messageId}`,
  content: { type: "text", text },
});

function tool(
  kind: string,
  title: string,
  extra: Update = {},
  status = "completed",
): Update {
  return {
    sessionUpdate: "tool_call",
    toolCallId: `tool-${++toolId}`,
    kind,
    title,
    status,
    ...extra,
  };
}

function edit(
  path: string,
  oldText: string,
  newText: string,
  status = "completed",
): Update {
  return tool(
    "edit",
    `Edit ${path.split("/").pop()}`,
    {
      locations: [{ path }],
      content: [{ type: "diff", path, oldText, newText }],
    },
    status,
  );
}

const RAIL = `${SIKEMUX}/src/rail/AgentRail.tsx`;
const SHELL_CSS = `${SIKEMUX}/src/styles/modern-shell.css`;

export interface AgentScript {
  history: Update[];
  live?: Update[];
  usage: { used: number; size: number };
}

export const AGENT_SCRIPTS: Record<string, AgentScript> = {
  [AGENTS.rail.id]: {
    usage: { used: 61_000, size: 200_000 },
    history: [
      user(
        "The compact rail density shifts agent labels about 2px right of the project rows. Fix it so every row keeps one inset and one gap, and add a test.",
      ),
      think(
        "The row rule lives in modern-shell.css; the compact density probably overrides only the leading inset. I should check where --mark and the gap are set.",
      ),
      tool("search", 'rg "rail-density" src', {
        rawOutput:
          "src/styles/modern-shell.css:1184\nsrc/rail/AgentRail.tsx:212",
      }),
      tool("read", "src/styles/modern-shell.css", {
        locations: [{ path: SHELL_CSS, line: 1170 }],
      }),
      tool("read", "src/rail/AgentRail.tsx", {
        locations: [{ path: RAIL, line: 205 }],
      }),
      say(
        "Found it. The compact density sets `padding-left: 6px` on agent rows but leaves the `8px` gap after the mark, so the label starts 2px further in than a project row's.\n\nThe inset and the gap should be the same step, so I'll drive both from one variable.",
      ),
      edit(
        SHELL_CSS,
        `.agent-rail[data-density="compact"] .agent-row {\n    padding-left: 6px;\n}`,
        `.agent-rail[data-density="compact"] .agent-row {\n    --row-inset: var(--space-1h);\n    padding-left: var(--row-inset);\n    gap: var(--row-inset);\n}`,
      ),
      edit(
        RAIL,
        `<div className="agent-row" data-state={state}>`,
        `<div className="agent-row" data-state={state} data-density={density}>`,
      ),
      tool("execute", "pnpm test src/rail/AgentRail", {
        rawOutput:
          " ✓ src/rail/AgentRail.test.tsx (24 tests) 311ms\n Test Files  1 passed (1)\n      Tests  24 passed (24)",
      }),
      say(
        "Labels now line up with the project rows at both densities. `AgentRail.test.tsx` passes, and I added a case that measures the label's left edge against a project row.",
      ),
      user(
        "Nice. Run the full rail suite and the visual check before I commit.",
      ),
    ],
    live: [
      think("Running the rail tests first, then the density snapshot at 2x."),
      tool(
        "execute",
        "pnpm test src/rail/AgentRail src/rail/SideRail",
        { rawOutput: " ✓ src/rail/AgentRail.test.tsx (25 tests) 318ms" },
      ),
      tool(
        "execute",
        "pnpm vitest run src/rail/AgentRailDensity.test.tsx",
        {},
        "in_progress",
      ),
    ],
  },
  [AGENTS.replay.id]: {
    usage: { used: 38_000, size: 272_000 },
    history: [
      user(
        "One of the terminal tests fails about one run in ten on CI. Find out why and fix it properly, without just adding a wait.",
      ),
      tool(
        "execute",
        "pnpm vitest run src/terminal/usePty.test.tsx --repeat 30",
        {
          rawOutput:
            "✗ replays the snapshot once (3/30 failed)\n  expected write to be called 1 time, but got 2 times",
        },
      ),
      think(
        "The screen is being written twice. That happens when new output arrives before the saved screen has finished loading.",
      ),
      tool("read", "src/terminal/ptyController.ts"),
      edit(
        `${SIKEMUX}/src/terminal/ptyController.ts`,
        "this.pending.push(chunk);",
        "if (chunk.byteLength > 0 && !this.replayed) return;\nthis.pending.push(chunk);",
      ),
      tool(
        "execute",
        "pnpm vitest run src/terminal/usePty.test.tsx --repeat 100",
        { rawOutput: "✓ replays the snapshot once (100/100)" },
      ),
      say(
        "New output could arrive while the saved screen was still loading, so the same text was shown twice. The terminal now waits for the saved screen first. The test passes 100 runs out of 100.",
      ),
      user(
        "Good. Run all the terminal tests, then commit it with a clear message.",
      ),
    ],
    live: [
      think(
        "Running every terminal test before I commit, since this change affects how terminals reconnect.",
      ),
      tool("execute", "pnpm vitest run src/terminal", {
        rawOutput: " Test Files  14 passed (14)\n      Tests  212 passed (212)",
        durationMs: 6_400,
      }),
      tool(
        "execute",
        "git add src/terminal/ptyController.ts src/terminal/usePty.test.tsx",
        { durationMs: 300 },
      ),
      tool(
        "execute",
        'git commit -m "fix(terminal): stop showing the same output twice after reconnecting"',
        {},
        "in_progress",
      ),
    ],
  },
  [AGENTS.hero.id]: {
    usage: { used: 44_000, size: 200_000 },
    history: [
      user(
        "On phones the Download button on the home page is cut off. Fix it, then check the page at phone size.",
      ),
      tool("read", "src/components/Hero.astro", {
        locations: [{ path: `${FRONT}/src/components/Hero.astro` }],
      }),
      say(
        "The two buttons sit side by side with a fixed width, so on a narrow screen the second one runs off the edge. I'll let them stack when there isn't room.",
      ),
      edit(
        `${FRONT}/src/components/Hero.astro`,
        ".actions {\n    display: flex;\n}",
        ".actions {\n    display: flex;\n    flex-wrap: wrap;\n    justify-content: center;\n}",
      ),
      tool("execute", "pnpm build", {
        rawOutput: "✓ Completed in 682ms.\n3 page(s) built",
      }),
      say(
        "The buttons now wrap onto two lines on small screens and stay side by side on wider ones. The site builds cleanly.",
      ),
      user("Show me the page at phone size."),
    ],
    live: [
      think("Opening the home page in my tab at phone width."),
      tool("fetch", "mcp__sikemux-tools__browser_navigate", {
        rawInput: { url: "http://localhost:4321/" },
        durationMs: 1_400,
      }),
      tool(
        "fetch",
        "mcp__sikemux-tools__browser_screenshot",
        {},
        "in_progress",
      ),
    ],
  },
  [AGENTS.incident.id]: {
    usage: { used: 61_000, size: 200_000 },
    history: [
      user(
        "Checkout has been returning 500s since this morning. Find out why, fix it, and update ACME-412.",
      ),
    ],
    live: [
      think("Reading the ticket and production logs, then checking the fix in the browser."),
      tool("read", "mcp__sikemux-tools__jira_issue", {
        rawInput: { key: "ACME-412" },
        durationMs: 700,
      }),
      tool("read", "mcp__sikemux-tools__signoz_logs", {
        rawInput: { service: "billing-service", severities: "error" },
        durationMs: 1_300,
      }),
      tool("execute", "mcp__sikemux-tools__task_start", {
        rawInput: { taskId: "dev" },
        durationMs: 400,
      }),
      tool("read", "mcp__sikemux-tools__task_read", {
        rawInput: { taskId: "dev", search: "ready" },
        durationMs: 2_100,
      }),
      tool("fetch", "mcp__sikemux-tools__browser_navigate", {
        rawInput: { url: "http://localhost:3000/checkout" },
        durationMs: 1_100,
      }),
      tool("edit", "mcp__sikemux-tools__browser_click", {
        rawInput: { text: "Pay now" },
        durationMs: 300,
      }),
      tool("read", "mcp__sikemux-tools__browser_screenshot", {
        rawInput: {},
        durationMs: 800,
      }),
      tool("edit", "mcp__sikemux-tools__jira_comment", {
        rawInput: { key: "ACME-412" },
        durationMs: 600,
      }),
      tool(
        "edit",
        "mcp__sikemux-tools__jira_transition",
        { rawInput: { key: "ACME-412", to: "In review" } },
        "in_progress",
      ),
    ],
  },
  [AGENTS.palette.id]: {
    usage: { used: 51_000, size: 272_000 },
    history: [
      user(
        "Compare median-cut and k-means for palette extraction on the 240-image set, and tell me which to ship.",
      ),
      tool("execute", "python -m benchmarks.palette --images 240", {
        rawOutput: "k-means (oklab)   66 ms   121 ms   2.3",
      }),
      say(
        "Ship k-means in OKLab. It is 25 ms slower at p50 than median-cut, but its palettes are 44% closer to the hand-picked references.",
      ),
    ],
  },
};

export const AVAILABLE_AGENTS = [
  {
    type: "claude",
    label: "Claude Code",
    command: "claude",
    available: true,
    defaultModel: "opus",
    status: { state: "ready", account: "subscription" },
    defaultEffort: "high",
  },
  {
    type: "codex",
    label: "Codex",
    command: "codex",
    available: true,
    defaultModel: "gpt-5.5",
    status: { state: "signedOut" },
    defaultEffort: "medium",
  },
  {
    type: "hermes",
    label: "Hermes",
    command: "hermes",
    available: true,
    defaultModel: null,
    status: { state: "unknown" },
    defaultEffort: null,
  },
  {
    type: "pi",
    label: "Pi",
    command: "pi",
    available: true,
    defaultModel: null,
    status: { state: "ready", account: null },
    defaultEffort: null,
  },
  {
    type: "opencode",
    label: "OpenCode",
    command: "opencode",
    available: true,
    defaultModel: null,
    status: { state: "ready", account: "apiKey" },
    defaultEffort: null,
  },
];

const hoursAgo = (hours: number) =>
  Math.floor(Date.now() / 1000 - hours * 3600);

export const SAVED_SESSIONS: Record<
  string,
  { id: string; title: string; mtime: number }[]
> = {
  claude: [
    {
      id: "s-c1",
      title: "Split the Rundeck matrix into its own pane",
      mtime: hoursAgo(3),
    },
    {
      id: "s-c2",
      title: "Why does the shader dim the gutter?",
      mtime: hoursAgo(26),
    },
    { id: "s-c3", title: "Release notes for v0.4.1", mtime: hoursAgo(49) },
  ],
  codex: [
    {
      id: "s-x1",
      title: "Benchmark CodeMirror virtualised scrolling",
      mtime: hoursAgo(5),
    },
    {
      id: "s-x2",
      title: "Port the git graph lanes to Rust",
      mtime: hoursAgo(30),
    },
  ],
};

const resetIn = (hours: number) => Math.floor(Date.now() / 1000 + hours * 3600);

export const AGENT_USAGE = {
  claude: {
    provider: "claude",
    plan: "Max",
    windows: [
      {
        label: "5h",
        usedPercent: 34,
        resetsAt: resetIn(2.3),
        windowMinutes: 300,
      },
      {
        label: "Week",
        usedPercent: 61,
        resetsAt: resetIn(76),
        windowMinutes: 10080,
      },
    ],
  },
  codex: {
    provider: "codex",
    plan: "Pro",
    windows: [
      {
        label: "5h",
        usedPercent: 12,
        resetsAt: resetIn(4.1),
        windowMinutes: 300,
      },
      {
        label: "Week",
        usedPercent: 27,
        resetsAt: resetIn(122),
        windowMinutes: 10080,
      },
    ],
  },
};

export const MODEL_OPTIONS: Record<string, unknown[]> = {
  claude: [
    {
      type: "select",
      id: "model",
      name: "Model",
      currentValue: "opus",
      options: [
        { value: "opus", name: "Opus 5.5" },
        { value: "sonnet", name: "Sonnet 5" },
      ],
    },
    {
      type: "select",
      id: "effort",
      name: "Effort",
      currentValue: "high",
      options: [
        { value: "medium", name: "Medium" },
        { value: "high", name: "High" },
      ],
    },
  ],
  codex: [
    {
      type: "select",
      id: "model",
      name: "Model",
      currentValue: "gpt-5.5",
      options: [{ value: "gpt-5.5", name: "GPT-5.5" }],
    },
    {
      type: "select",
      id: "reasoning_effort",
      name: "Effort",
      currentValue: "medium",
      options: [
        { value: "medium", name: "Medium" },
        { value: "high", name: "High" },
      ],
    },
  ],
};
