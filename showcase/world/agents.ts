import { AGENTS } from "./workspace";
import { SIKEMUX } from "./projects";

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

const RAIL = `${SIKEMUX}/src/components/AgentRail.tsx`;
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
          "src/styles/modern-shell.css:1184\nsrc/components/AgentRail.tsx:212",
      }),
      tool("read", "src/styles/modern-shell.css", {
        locations: [{ path: SHELL_CSS, line: 1170 }],
      }),
      tool("read", "src/components/AgentRail.tsx", {
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
      tool("execute", "pnpm test src/components/AgentRail", {
        rawOutput:
          " ✓ src/components/AgentRail.test.tsx (24 tests) 311ms\n Test Files  1 passed (1)\n      Tests  24 passed (24)",
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
        "pnpm test src/components/AgentRail src/components/SideRail",
        { rawOutput: " ✓ src/components/AgentRail.test.tsx (25 tests) 318ms" },
      ),
      tool(
        "execute",
        "pnpm vitest run src/components/AgentRailDensity.test.tsx",
        {},
        "in_progress",
      ),
    ],
  },
  [AGENTS.replay.id]: {
    usage: { used: 38_000, size: 272_000 },
    history: [
      user(
        "`usePty.test.tsx > replays the snapshot once` fails about one run in ten on CI. Find out why and fix it without adding sleeps.",
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
        "Two writes means the attach response and the first channel message both carry the snapshot. That happens when the channel is created before attach resolves.",
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
        "The channel could deliver bytes that were already inside the attach snapshot. The controller now drops channel output until the snapshot has been written, which passes 100 out of 100 runs.",
      ),
      user(
        "Good. Run the whole terminal suite, then commit it with a message that explains the race.",
      ),
    ],
    live: [
      think(
        "Running every terminal test before the commit, since the controller change touches attach and detach.",
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
        'git commit -m "fix(pty): drop channel bytes the attach snapshot already holds"',
        {},
        "in_progress",
      ),
    ],
  },
  [AGENTS.hero.id]: {
    usage: { used: 44_000, size: 200_000 },
    history: [
      user(
        "Hermes shows as unavailable every time it updates, then comes back later on its own. Find out why, fix it, and open a PR.",
      ),
      tool("read", "src-tauri/src/agents.rs", {
        locations: [{ path: `${SIKEMUX}/src-tauri/src/agents.rs` }],
      }),
      tool("execute", "time hermes --version", {
        rawOutput: "hermes 0.9.4\nreal 0m9.84s",
      }),
      say(
        "`hermes --version` runs an update check that fetches from upstream, bounded by a 10s timeout and cached for six hours. Our probe gives up after 2s and retries for 8s, so every cache expiry made an installed Hermes look unavailable.",
      ),
      edit(
        `${SIKEMUX}/src-tauri/src/agents.rs`,
        'let probe = ["--version"];',
        'let probe = ["--help"];',
      ),
      tool(
        "execute",
        "cargo test --manifest-path src-tauri/Cargo.toml agents",
        {
          rawOutput: "test result: ok. 38 passed; 0 failed",
        },
      ),
      tool(
        "execute",
        'gh pr create --title "fix(agents): probe Hermes with --help, not --version" --fill',
        {
          rawOutput: "https://github.com/nodelike/sikemux/pull/14",
        },
      ),
      tool("execute", "gh pr merge 14 --squash --delete-branch", {
        rawOutput: "✓ Squashed and merged pull request nodelike/sikemux#14",
      }),
      say(
        "`--help` takes Hermes' fast path: no network, exits 0, and still runs the same wrapper and interpreter that launching the agent does. Opened and merged #14.",
      ),
      user("Show me the PR."),
    ],
    live: [
      think("Opening the pull request in my tab."),
      tool("fetch", "mcp__sikemux-tools__browser_navigate", {
        rawInput: { url: "https://github.com/nodelike/sikemux/pull/14" },
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
    defaultEffort: "high",
  },
  {
    type: "codex",
    label: "Codex",
    command: "codex",
    available: true,
    defaultModel: "gpt-5.5",
    defaultEffort: "medium",
  },
  {
    type: "hermes",
    label: "Hermes",
    command: "hermes",
    available: true,
    defaultModel: null,
    defaultEffort: null,
  },
  {
    type: "pi",
    label: "Pi",
    command: "pi",
    available: true,
    defaultModel: null,
    defaultEffort: null,
  },
  {
    type: "opencode",
    label: "OpenCode",
    command: "opencode",
    available: true,
    defaultModel: null,
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
