import { AGENTS } from "./workspace";
import { FRONT, MOODBOARD, SIKEMUX } from "./projects";

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

const RAIL = `${SIKEMUX}/src/rail/TabRail.tsx`;
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
        rawOutput: "src/styles/modern-shell.css:1184\nsrc/rail/TabRail.tsx:212",
      }),
      tool("read", "src/styles/modern-shell.css", {
        locations: [{ path: SHELL_CSS, line: 1170 }],
      }),
      tool("read", "src/rail/TabRail.tsx", {
        locations: [{ path: RAIL, line: 205 }],
      }),
      say(
        "Found it. The compact density sets `padding-left: 6px` on agent rows but leaves the `8px` gap after the mark, so the label starts 2px further in than a project row's.\n\nThe inset and the gap should be the same step, so I'll drive both from one variable.",
      ),
      edit(
        SHELL_CSS,
        `.tab-rail[data-density="compact"] .agent-row {\n    padding-left: 6px;\n}`,
        `.tab-rail[data-density="compact"] .agent-row {\n    --row-inset: var(--space-1h);\n    padding-left: var(--row-inset);\n    gap: var(--row-inset);\n}`,
      ),
      edit(
        RAIL,
        `<div className="agent-row" data-state={state}>`,
        `<div className="agent-row" data-state={state} data-density={density}>`,
      ),
      tool("execute", "pnpm test src/rail/TabRail", {
        rawOutput:
          " ✓ src/rail/TabRail.test.tsx (24 tests) 311ms\n Test Files  1 passed (1)\n      Tests  24 passed (24)",
      }),
      say(
        "Labels now line up with the project rows at both densities. `TabRail.test.tsx` passes, and I added a case that measures the label's left edge against a project row.",
      ),
      user(
        "Nice. Run the full rail suite and the visual check before I commit.",
      ),
    ],
    live: [
      think("Running the rail tests first, then the density snapshot at 2x."),
      tool("execute", "pnpm test src/rail/TabRail src/rail/SideRail", {
        rawOutput: " ✓ src/rail/TabRail.test.tsx (25 tests) 318ms",
      }),
      tool(
        "execute",
        "pnpm vitest run src/rail/TabRailDensity.test.tsx",
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
  [AGENTS.phone.id]: {
    usage: { used: 52_000, size: 200_000 },
    history: [
      user(
        "On the phone, the permission card should make Allow the obvious choice. Build it for the simulator and check it.",
      ),
      say(
        "Allow is now the filled button, with Deny and Always allow beside it as outlines. Building the app, then I'll check the card on an iPhone.",
      ),
    ],
    live: [
      tool("execute", "pnpm ios:build --simulator", {
        rawOutput: "› Build succeeded · SikemuxDev.app",
        durationMs: 41_000,
      }),
      tool("move", "mcp__sikemux-tools__sim_attach", {
        rawInput: { device: "iPhone 18 Pro" },
        durationMs: 2_100,
      }),
      tool("execute", "mcp__sikemux-tools__sim_install", {
        rawInput: { path: "build/SikemuxDev.app" },
        durationMs: 3_400,
      }),
      tool("execute", "mcp__sikemux-tools__sim_launch", {
        rawInput: { bundleId: "com.nodelike.sikemux.dev" },
        durationMs: 1_800,
      }),
      tool("edit", "mcp__sikemux-tools__sim_tap", {
        rawInput: { label: "Add dark mode to settings" },
        durationMs: 400,
      }),
      tool("edit", "mcp__sikemux-tools__sim_swipe", {
        rawInput: { fromX: 201, fromY: 640, toX: 201, toY: 320 },
        durationMs: 600,
      }),
      tool("read", "mcp__sikemux-tools__sim_screenshot", {}, "in_progress"),
    ],
  },
  [AGENTS.hero.id]: {
    usage: { used: 44_000, size: 200_000 },
    history: [
      user(
        "The Read the issue button runs off the edge of the cover when the window is narrow. Fix it, then check the home page.",
      ),
      tool("read", "src/sections/Cover.svelte", {
        locations: [{ path: `${MOODBOARD}/src/sections/Cover.svelte` }],
      }),
      say(
        "Shop the drop and Read the issue sit side by side at a fixed width, so in a narrow window the second one runs off the edge. I'll let them share the row and wrap when there isn't room.",
      ),
      edit(
        `${MOODBOARD}/src/sections/Cover.svelte`,
        ".actions {\n    display: flex;\n}\n.btn {\n    width: 180px;\n}",
        ".actions {\n    display: flex;\n    flex-wrap: wrap;\n    gap: 8px;\n}\n.btn {\n    flex: 1 1 auto;\n    min-width: 120px;\n}",
      ),
      tool("execute", "pnpm build", {
        rawOutput: "✓ built in 1.48s",
      }),
      say(
        "Both buttons now share the row and wrap when the window gets narrow. The site builds cleanly.",
      ),
      user("Nice. Open vercel.com in your tab."),
    ],
    live: [
      think("Opening vercel.com in my tab."),
      tool("fetch", "mcp__sikemux-tools__browser_navigate", {
        rawInput: { url: "https://vercel.com/" },
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
      think(
        "Reading the ticket and production logs, then checking the fix in the browser.",
      ),
      tool("read", "mcp__sikemux-tools__jira_issue", {
        rawInput: { key: "ACME-412" },
        rawOutput:
          "ACME-412 · Checkout returns 500 for saved cards\nIn Progress · High · reported by Edon Ramos\n\nSince 09:12 about one checkout in twenty fails with a 500 when paying with a saved card. New cards go through.",
        durationMs: 700,
      }),
      tool("read", "mcp__sikemux-tools__signoz_logs", {
        rawInput: { service: "billing-service", severities: "error" },
        rawOutput:
          "09:12:04  ERROR  charge failed: card token signed with retired key v2\n09:12:31  ERROR  POST /checkout 500 · 212 ms\n09:13:02  ERROR  charge failed: card token signed with retired key v2\n09:13:40  ERROR  POST /checkout 500 · 198 ms\n… 118 more in the last hour",
        durationMs: 1_300,
      }),
      say(
        "Saved cards still carry tokens signed with the retired v2 key, so the charge call refuses them and checkout answers 500. I'll make the charge read both keys, then check it in the browser.",
      ),
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
  [AGENTS.flow.id]: {
    usage: { used: 38_000, size: 200_000 },
    history: [
      user(
        "Draw how a checkout request moves through our services, and mark where the 500s start.",
      ),
      say(
        "Checkout goes through the gateway to billing-service, which calls payments, orders and the email worker. The 500s start at payments: it now takes 3.4 s at p95, past billing's 3 s timeout.",
      ),
      tool("other", "mcp__sikemux-tools__page_show", {
        rawInput: { title: "How a checkout request flows" },
        rawOutput: JSON.stringify({
          page: {
            id: "a1c4e7f20b9d4c3e8f5a6b7c8d9e0f11",
            title: "How a checkout request flows",
            height: 330,
          },
        }),
        durationMs: 300,
      }),
    ],
  },
  [AGENTS.latency.id]: {
    usage: { used: 38_000, size: 200_000 },
    history: [
      user(
        "Did the fix bring checkout latency back down? Show me the last 24 hours.",
      ),
      say(
        "Yes. p95 dropped from 3.1 s to 390 ms right after the 17:05 deploy, and no checkout has failed since.",
      ),
      tool("other", "mcp__sikemux-tools__page_show", {
        rawInput: { title: "checkout p95 latency, last 24 hours" },
        rawOutput: JSON.stringify({
          page: {
            id: "b2d5f8a31c0e4d5f9a6b7c8d9e0f1a22",
            title: "checkout p95 latency, last 24 hours",
            height: 360,
          },
        }),
        durationMs: 300,
      }),
    ],
  },
  [AGENTS.empty.id]: {
    usage: { used: 38_000, size: 200_000 },
    history: [
      user(
        "Sketch three options for the empty projects page before I pick one.",
      ),
      say("Here are three, drawn with the site's colours. I'd go with B."),
      tool("other", "mcp__sikemux-tools__page_show", {
        rawInput: { title: "Empty projects page, three ways" },
        rawOutput: JSON.stringify({
          page: {
            id: "c3e6a9b42d1f4e6a8b7c9d0e1f2a3b33",
            title: "Empty projects page, three ways",
            height: 330,
          },
        }),
        durationMs: 300,
      }),
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

const letter = (from: { id: string; name: string; type: string }, text: string) =>
  user(`<agent-message from="${from.name}" agent-id="${from.id}" cli="${from.type}">\n${text}\n</agent-message>`);

const COVER = {
  url: "http://localhost:5173/",
  title: "Moodboard Studio · Issue 07",
  picture: "/showcase/pages/moodboard-cover.png",
};
const SHOP = {
  url: "http://localhost:5173/shop",
  title: "Shop the drop · Moodboard Studio",
  picture: "/showcase/pages/moodboard-shop.png",
};
export const COVER_PROMPT = "Open the dev server in your tab and show me Shop the drop.";
const coverReply = say("The cover buttons share the row now, and Shop the drop opens the shop:");
const railTests = tool(
  "execute",
  "pnpm test src/rail --run",
  { rawInput: { command: "pnpm test src/rail --run" }, asks: true, durationMs: 600 },
  "in_progress",
);

const wrenChecksTheCover: Update[] = [
  think("Opening the cover in my tab first."),
  tool("fetch", "mcp__sikemux-tools__browser_navigate", {
    rawInput: { url: COVER.url },
    browser: COVER,
    durationMs: 1_600,
  }),
  tool("edit", "mcp__sikemux-tools__browser_click", {
    rawInput: { text: "Shop the drop" },
    browser: SHOP,
    durationMs: 1_400,
  }),
  tool("read", "mcp__sikemux-tools__browser_screenshot", { rawInput: {}, durationMs: 900 }),
  coverReply,
  {
    sessionUpdate: "agent_message_chunk",
    messageId: coverReply.messageId,
    content: { type: "image", uri: SHOP.picture },
  },
];

const cardReply = say("Allow is the filled button now, with Deny and Always allow beside it as outlines:");

/**
 * A turn a video plays when it says so, after the chat has opened. A step's
 * `board` changes a card, `browser` moves the agent's tab to a page, and
 * `asks` makes a command wait for the person's permission.
 */
export const CUED_TURNS: Record<string, { agentId: string; updates: Update[] }> = {
  "wren-checks-the-cover": { agentId: AGENTS.hero.id, updates: wrenChecksTheCover },
  "wren-is-asked-to-check-the-cover": {
    agentId: AGENTS.hero.id,
    updates: [user(COVER_PROMPT), ...wrenChecksTheCover],
  },
  "comet-checks-the-card": {
    agentId: AGENTS.phone.id,
    updates: [
      think("Installing the build on the simulator, then opening a chat that asks for permission."),
      tool("execute", "mcp__sikemux-tools__sim_install", {
        rawInput: { path: "build/SikemuxDev.app" },
        durationMs: 1_200,
      }),
      tool("execute", "mcp__sikemux-tools__sim_launch", {
        rawInput: { bundleId: "com.nodelike.sikemux.dev" },
        simulator: "home",
        durationMs: 1_500,
      }),
      tool("edit", "mcp__sikemux-tools__sim_tap", {
        rawInput: { label: "MacBook Pro" },
        simulator: "device",
        durationMs: 1_500,
      }),
      tool("edit", "mcp__sikemux-tools__sim_tap", {
        rawInput: { label: "Add dark mode to settings" },
        simulator: "asking",
        durationMs: 1_600,
      }),
      tool("read", "mcp__sikemux-tools__sim_screenshot", { rawInput: {}, durationMs: 800 }),
      cardReply,
      {
        sessionUpdate: "agent_message_chunk",
        messageId: cardReply.messageId,
        content: { type: "image", uri: "/__showcase/simulator/frame/asking" },
      },
    ],
  },
  "flint-charts-the-palettes": {
    agentId: AGENTS.palette.id,
    updates: [
      user("How much closer? Show me."),
      think("Plotting each category's distance from the hand-picked palettes."),
      say("Closer in every category, and by the most on night shots, for 25 ms more at p50:"),
      tool("other", "mcp__sikemux-tools__page_show", {
        rawInput: { title: "Palette distance from the hand-picked references" },
        rawOutput: JSON.stringify({
          page: {
            id: "d4e7b1c53a2f4b7c9d8e0f1a2b3c4d44",
            title: "Palette distance from the hand-picked references",
            height: 340,
          },
        }),
        durationMs: 500,
      }),
    ],
  },
  "otter-runs-the-rail-tests": {
    agentId: AGENTS.rail.id,
    updates: [
      think("The rail suite needs a shell, so I'll ask before running it."),
      railTests,
      { sessionUpdate: "tool_call_update", toolCallId: railTests.toolCallId, status: "in_progress", durationMs: 1_800 },
      {
        sessionUpdate: "tool_call_update",
        toolCallId: railTests.toolCallId,
        status: "completed",
        rawOutput: " ✓ src/rail (41 tests) 2.14s\n Test Files  9 passed (9)",
      },
      say("All 41 rail tests pass, so the labels are ready to commit."),
    ],
  },
  "otter-asks-maple": {
    agentId: AGENTS.rail.id,
    updates: [
      think("The refunds bug waits on Maple's ledger import, so I'll check where that is first."),
      tool("other", "mcp__sikemux-tools__agent_send", {
        rawInput: {
          to: AGENTS.replay.name,
          text: "Is the ledger import onto amounts ready to land? I'll take the refunds bug as soon as it is.",
        },
        durationMs: 3200,
      }),
      say("I've asked Maple. The refunds bug waits on the ledger import, so I'll pick it up once that lands."),
    ],
  },
  "maple-answers": {
    agentId: AGENTS.replay.id,
    updates: [
      letter(
        AGENTS.rail,
        "Is the ledger import onto amounts ready to land? I'll take the refunds bug as soon as it is.",
      ),
      think("Otter is waiting on the import. One more run of the ledger tests before I close the card."),
      tool("execute", "pnpm vitest run src/billing/ledger", {
        rawOutput: " ✓ src/billing/ledger.test.ts (18 tests) 204ms\n Test Files  1 passed (1)",
        durationMs: 1600,
      }),
      tool("other", "mcp__sikemux-tools__agent_send", {
        rawInput: {
          to: AGENTS.rail.name,
          text: "Landed. The import reads amounts in cents and all 18 ledger tests pass, so the refunds bug is yours.",
        },
      }),
      say("Told Otter the import is in. Closing the card."),
      tool("edit", "mcp__sikemux-tools__board_update", {
        rawInput: {
          card: 3,
          status: "done",
          result: "Ledger rows import onto amounts in cents; 18 tests pass.",
        },
        board: { id: 3, status: "done", result: "Ledger rows import onto amounts in cents; 18 tests pass." },
      }),
    ],
  },
  "otter-takes-refunds": {
    agentId: AGENTS.rail.id,
    updates: [
      letter(
        AGENTS.replay,
        "Landed. The import reads amounts in cents and all 18 ledger tests pass, so the refunds bug is yours.",
      ),
      tool("edit", "mcp__sikemux-tools__board_claim", {
        rawInput: { card: 6 },
        board: {
          id: 6,
          status: "progress",
          owner: AGENTS.rail.id,
          ownerName: AGENTS.rail.name,
          ownerCli: AGENTS.rail.type,
          team: AGENTS.rail.id,
          teamName: AGENTS.rail.name,
          waitsOn: [],
        },
      }),
      say("Maple's import is in, so I've taken the refunds bug. Starting with the refund path's rounding."),
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

/** `said` stands in for what the person and the agent wrote, which the core's search also reads. */
export const SAVED_SESSIONS: Record<
  string,
  { id: string; title: string; mtime: number; project: string; said?: string }[]
> = {
  claude: [
    {
      id: "s-c1",
      title: "Split the Rundeck matrix into its own pane",
      project: SIKEMUX,
      mtime: hoursAgo(3),
      said: "Each job's matrix of nodes gets a pane of its own, with the log tail under it.",
    },
    {
      id: "s-c2",
      title: "Why does the shader dim the gutter?",
      project: SIKEMUX,
      mtime: hoursAgo(26),
      said: "The gutter sits under the vignette, so its alpha is multiplied twice.",
    },
    {
      id: "s-c3",
      title: "Release notes for v0.4.1",
      project: SIKEMUX,
      mtime: hoursAgo(49),
    },
    {
      id: "s-c4",
      title: "Halftone the lookbook photos without banding",
      project: MOODBOARD,
      mtime: hoursAgo(2),
      said: "Dither before the halftone pass, so the dots stop stepping in the shadows.",
    },
    {
      id: "s-c5",
      title: "Pull a palette from each moodboard",
      project: MOODBOARD,
      mtime: hoursAgo(20),
    },
    {
      id: "s-c6",
      title: "Lazy-load the board grid past the fold",
      project: MOODBOARD,
      mtime: hoursAgo(45),
    },
    {
      id: "s-c21",
      title: "Snap cards to the board grid while dragging",
      project: MOODBOARD,
      mtime: hoursAgo(50),
    },
    {
      id: "s-c22",
      title: "Why does the colour picker drift on P3 screens?",
      project: MOODBOARD,
      mtime: hoursAgo(70),
    },
    {
      id: "s-c23",
      title: "Share a board as a read-only link",
      project: MOODBOARD,
      mtime: hoursAgo(76),
    },
    {
      id: "s-c24",
      title: "Undo for deleting a whole column",
      project: MOODBOARD,
      mtime: hoursAgo(98),
    },
    {
      id: "s-c25",
      title: "Blur placeholders while images load",
      project: MOODBOARD,
      mtime: hoursAgo(120),
    },
    {
      id: "s-c26",
      title: "Tag boards by season and collection",
      project: MOODBOARD,
      mtime: hoursAgo(145),
    },
    {
      id: "s-c27",
      title: "Import a Pinterest board by URL",
      project: MOODBOARD,
      mtime: hoursAgo(170),
    },
    {
      id: "s-c28",
      title: "Fix the zoom jump when pinching on a trackpad",
      project: MOODBOARD,
      mtime: hoursAgo(196),
    },
    {
      id: "s-c29",
      title: "Keyboard shortcuts for the board editor",
      project: MOODBOARD,
      mtime: hoursAgo(220),
    },
    {
      id: "s-c30",
      title: "Compress uploads before they reach the bucket",
      project: MOODBOARD,
      mtime: hoursAgo(260),
    },
    {
      id: "s-c31",
      title: "Rail rows line up with project names",
      project: SIKEMUX,
      mtime: hoursAgo(75),
    },
    {
      id: "s-c32",
      title: "Notch island shows the waiting agent first",
      project: SIKEMUX,
      mtime: hoursAgo(98),
    },
    {
      id: "s-c33",
      title: "Terminal search highlights every match",
      project: SIKEMUX,
      mtime: hoursAgo(122),
    },
    {
      id: "s-c34",
      title: "Keep the browser tab awake while an agent drives it",
      project: SIKEMUX,
      mtime: hoursAgo(150),
    },
    {
      id: "s-c35",
      title: "Diff view folds unchanged hunks",
      project: SIKEMUX,
      mtime: hoursAgo(175),
    },
    {
      id: "s-c36",
      title: "Command deck remembers the last query",
      project: SIKEMUX,
      mtime: hoursAgo(200),
    },
    {
      id: "s-c37",
      title: "Theme picker previews before it applies",
      project: SIKEMUX,
      mtime: hoursAgo(230),
    },
    {
      id: "s-c38",
      title: "Speed up opening a pane in a large repo",
      project: SIKEMUX,
      mtime: hoursAgo(262),
    },
    {
      id: "s-c39",
      title: "Phone pairing card reads the host's name",
      project: SIKEMUX,
      mtime: hoursAgo(290),
    },
    {
      id: "s-c40",
      title: "Hero picture on an ultrawide screen",
      project: FRONT,
      mtime: hoursAgo(70),
    },
    {
      id: "s-c41",
      title: "Install line under both download buttons",
      project: FRONT,
      mtime: hoursAgo(95),
    },
    {
      id: "s-c42",
      title: "Feature cards cut from the showcase",
      project: FRONT,
      mtime: hoursAgo(130),
    },
    {
      id: "s-c43",
      title: "Dark social card for the release post",
      project: FRONT,
      mtime: hoursAgo(160),
    },
    {
      id: "s-c44",
      title: "Size note next to the download",
      project: FRONT,
      mtime: hoursAgo(190),
    },
    {
      id: "s-c45",
      title: "Changelog page reads the GitHub releases",
      project: FRONT,
      mtime: hoursAgo(230),
    },
    {
      id: "s-c50",
      title: "Print styles for the lookbook export",
      project: MOODBOARD,
      mtime: hoursAgo(290),
    },
    {
      id: "s-c51",
      title: "Pick a cover image for each board",
      project: MOODBOARD,
      mtime: hoursAgo(320),
    },
    {
      id: "s-c52",
      title: "Migrate boards to the new storage layout",
      project: MOODBOARD,
      mtime: hoursAgo(350),
    },
    {
      id: "s-c53",
      title: "Fonts render too thin in Safari",
      project: MOODBOARD,
      mtime: hoursAgo(380),
    },
    {
      id: "s-c54",
      title: "Arrange boards by colour",
      project: MOODBOARD,
      mtime: hoursAgo(410),
    },
    {
      id: "s-c7",
      title: "Rewrite the pricing table for the new plans",
      project: FRONT,
      mtime: hoursAgo(8),
    },
    {
      id: "s-c8",
      title: "Make the download button read the latest release",
      project: FRONT,
      mtime: hoursAgo(52),
      said: "The button finds the DMG by its _aarch64.dmg suffix on the latest release.",
    },
  ],
  codex: [
    {
      id: "s-x1",
      title: "Benchmark CodeMirror virtualised scrolling",
      project: SIKEMUX,
      mtime: hoursAgo(5),
    },
    {
      id: "s-x2",
      title: "Port the git graph lanes to Rust",
      project: SIKEMUX,
      mtime: hoursAgo(30),
    },
    {
      id: "s-x3",
      title: "Fix the OAuth callback after sign-in",
      project: SIKEMUX,
      mtime: hoursAgo(73),
      said: "The callback port was taken by a stale listener, so the redirect never landed.",
    },
    {
      id: "s-x4",
      title: "Export a board as a print-ready PDF",
      project: MOODBOARD,
      mtime: hoursAgo(9),
    },
    {
      id: "s-x5",
      title: "Drop the unused fonts from the board editor",
      project: MOODBOARD,
      mtime: hoursAgo(60),
    },
    {
      id: "s-x6",
      title: "Cache the release lookup at build time",
      project: FRONT,
      mtime: hoursAgo(28),
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
      category: "thought_level",
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
      category: "thought_level",
      name: "Effort",
      currentValue: "medium",
      options: [
        { value: "medium", name: "Medium" },
        { value: "high", name: "High" },
      ],
    },
  ],
};
