import type { Workspace } from "@shared/wire.generated";
import type { Agent, LayoutNode, Session, Window } from "../../src/state/types";
import { VERSION, type PersistedSnapshot } from "../../src/state/persist";
import {
  DEMO_HOME,
  FRONT,
  MOODBOARD,
  SIKEMUX,
} from "./projects";

const pane = (
  id: string,
  cwd: string,
  kind: "terminal" | "editor" | "git" | "agent",
  title: string,
): LayoutNode => ({ type: "pane", id, cwd, kind, title });

const split = (
  id: string,
  dir: "row" | "column",
  children: LayoutNode[],
  sizes: number[],
): LayoutNode => ({ type: "split", id, dir, children, sizes });

const terminalWindow = (
  id: string,
  root: LayoutNode,
  activePaneId: string,
): Window => ({ id, name: "Terminal", role: "term", root, activePaneId });

const agentWindow = (agent: Agent): Window => ({
  id: `w-${agent.id}`,
  name: agent.title,
  role: "agent",
  root: pane(agent.id, agent.cwd ?? SIKEMUX, "agent", agent.title),
  activePaneId: agent.id,
});

export const AGENTS = {
  rail: {
    id: "tab-rail",
    name: "Otter",
    type: "claude",
    title: "Line up the sidebar labels",
    resumeId: "5f0c8a1e-rail",
    cwd: SIKEMUX,
  },
  replay: {
    id: "agent-replay",
    name: "Maple",
    type: "codex",
    title: "Fix a flaky terminal test",
    resumeId: "codex-replay-0192",
    cwd: SIKEMUX,
  },
  hero: {
    id: "agent-hero",
    name: "Wren",
    type: "claude",
    title: "Fix the cover buttons",
    resumeId: "91d2e7b4-hero",
    cwd: MOODBOARD,
  },
  palette: {
    id: "agent-palette",
    name: "Flint",
    type: "codex",
    title: "Compare two ways to pick colours",
    resumeId: "codex-palette-7731",
    cwd: MOODBOARD,
  },
  notes: {
    id: "agent-notes",
    name: "Heron",
    type: "hermes",
    title: "Write up the colour results",
    resumeId: "hermes-notes-3310",
    cwd: MOODBOARD,
  },
  flow: {
    id: "agent-flow",
    name: "Birch",
    type: "claude",
    title: "Map the checkout flow",
    resumeId: "c41f2b90-flow",
    cwd: `${DEMO_HOME}/work/billing-service`,
  },
  latency: {
    id: "agent-latency",
    name: "Cedar",
    type: "codex",
    title: "Check checkout latency after the fix",
    resumeId: "codex-latency-5521",
    cwd: `${DEMO_HOME}/work/billing-service`,
  },
  empty: {
    id: "agent-empty",
    name: "Juniper",
    type: "claude",
    title: "Empty state options",
    resumeId: "91d2e7b4-empty",
    cwd: FRONT,
  },
  phone: {
    id: "agent-phone",
    name: "Comet",
    type: "claude",
    title: "Check the permission card on iPhone",
    resumeId: "5f0c8a1e-phone",
    cwd: SIKEMUX,
  },
  incident: {
    id: "agent-incident",
    name: "Moss",
    type: "claude",
    title: "Fix the checkout 500s",
    resumeId: "c41f2b90-incident",
    cwd: `${DEMO_HOME}/work/billing-service`,
  },
} satisfies Record<string, Agent>;

const agents: Agent[] = Object.values(AGENTS).map((agent) => ({
  ...agent,
  permissionMode:
    agent.id === "agent-replay" || agent.id === "agent-hero"
      ? "bypass"
      : "workspace-write",
  keepAlive: true,
}));

export const EDITOR_TABS = [`${SIKEMUX}/src/rail/TabRail.tsx`];

/** Terminals that only fill the rail out, each opening on one shell. */
const MORE_TERMINALS = [
  { id: "s-logs", name: "logs" },
  { id: "s-scratch", name: "scratch" },
];

const oneTerminal = (
  sessionId: string,
  cwd: string,
  title: string,
): Window[] => [
  terminalWindow(
    `w-${sessionId}`,
    pane(`t-${sessionId}`, cwd, "terminal", title),
    `t-${sessionId}`,
  ),
];

const PROJECT_SPACES: Record<string, string> = {
  [SIKEMUX]: "space-personal",
  [FRONT]: "space-personal",
  [MOODBOARD]: "space-personal",
  [`${DEMO_HOME}/work/billing-service`]: "space-acme",
};

/** The workspace the demo's core keeps: its projects, spaces and agents. */
export function demoWorkspace(): Workspace {
  const projects = baseSnapshot().sessions.filter(
    (session): session is Session =>
      session.kind === "project" && "cwd" in session,
  );
  return {
    projects: projects.map((session) => ({
      id: session.id,
      name: session.name,
      path: session.cwd,
      pinned: session.pinned,
      space: PROJECT_SPACES[session.cwd],
    })),
    spaces: [
      { id: "space-acme", name: "Acme", icon: "building" },
      { id: "space-personal", name: "Personal", icon: "leaf" },
    ],
    agents: agents.map((agent) => ({
      ...agent,
      provider: agent.type,
      project:
        projects.find((session) => session.cwd === agent.cwd)?.id ??
        "s-sikemux",
      cwd: agent.cwd ?? SIKEMUX,
      renamed: false,
      permissionMode: agent.permissionMode ?? "workspace-write",
      keepAlive: agent.keepAlive === true,
      asleep: false,
      createdAt: 0,
    })),
    pinned: [],
    permissionMode: "bypass",
    disabledPlugins: [],
    launchers: [],
    palette: {},
    backdrop: { texture: false, image: null },
  };
}

export function demoSnapshot(): PersistedSnapshot {
  const snapshot = baseSnapshot();
  for (const terminal of MORE_TERMINALS) {
    snapshot.sessions.push({
      id: terminal.id,
      name: terminal.name,
      kind: "command",
      cwd: DEMO_HOME,
      pinned: false,
      activeWindowId: `w-${terminal.id}`,
    });
    snapshot.windowsBySession[terminal.id] = oneTerminal(
      terminal.id,
      DEMO_HOME,
      "zsh",
    );
  }
  snapshot.sessionOrder.push(...MORE_TERMINALS.map((terminal) => terminal.id));
  return snapshot;
}

function baseSnapshot(): PersistedSnapshot {
  return {
    version: VERSION,
    sessions: [
      {
        id: "s-sikemux",
        name: "sikemux",
        kind: "project",
        cwd: SIKEMUX,
        pinned: false,
        activeWindowId: `w-${AGENTS.rail.id}`,
      },
      {
        id: "s-front",
        name: "sikemux-front",
        kind: "project",
        cwd: FRONT,
        pinned: false,
        activeWindowId: "w-front-term",
      },
      {
        id: "s-mood",
        name: "moodboard-studio",
        kind: "project",
        cwd: MOODBOARD,
        pinned: false,
        activeWindowId: "w-mood-term",
      },
      {
        id: "s-billing",
        name: "billing-service",
        kind: "project",
        cwd: `${DEMO_HOME}/work/billing-service`,
        pinned: false,
        activeWindowId: "w-billing-service",
      },
      {
        id: "s-gpu",
        name: "gpu-box",
        kind: "ssh",
        cwd: "",
        pinned: false,
        activeWindowId: "w-gpu",
      },
      {
        id: "s-bastion",
        name: "staging-bastion",
        kind: "ssh",
        cwd: "",
        pinned: false,
        activeWindowId: "w-bastion",
      },
      {
        id: "s-runner",
        name: "build-runner",
        kind: "ssh",
        cwd: "",
        pinned: false,
        activeWindowId: "w-runner",
      },
      {
        id: "s-replica",
        name: "db-replica",
        kind: "ssh",
        cwd: "",
        pinned: false,
        activeWindowId: "w-replica",
      },
      {
        id: "s-pihole",
        name: "pi-hole",
        kind: "ssh",
        cwd: "",
        pinned: false,
        activeWindowId: "w-pihole",
      },
      {
        id: "s-shell",
        name: "zsh",
        kind: "command",
        cwd: DEMO_HOME,
        pinned: false,
        activeWindowId: "w-shell",
      },
      {
        id: "s-rundeck",
        name: "Rundeck",
        kind: "sikemux.rundeck:deploy",
        cwd: "",
        pinned: false,
        activeWindowId: "w-rundeck",
      },
      {
        id: "s-signoz",
        name: "SigNoz",
        kind: "sikemux.signoz:explore",
        cwd: "",
        pinned: false,
        activeWindowId: "w-signoz",
      },
      {
        id: "s-database",
        name: "Database",
        kind: "sikemux.database:browser",
        cwd: "",
        pinned: false,
        activeWindowId: "w-database",
      },
      {
        id: "s-jira",
        name: "Jira",
        kind: "sikemux.jira:issues",
        cwd: "",
        pinned: false,
        activeWindowId: "w-jira",
      },
    ],
    windowsBySession: {
      "s-sikemux": [
        terminalWindow(
          "w-sikemux-term",
          split(
            "sp-sikemux",
            "row",
            [
              pane("t-dev", SIKEMUX, "terminal", "dev"),
              split(
                "sp-sikemux-right",
                "column",
                [
                  pane("t-test", SIKEMUX, "terminal", "test"),
                  pane("t-git", SIKEMUX, "terminal", "git"),
                ],
                [0.5, 0.5],
              ),
            ],
            [0.5, 0.5],
          ),
          "t-dev",
        ),
        terminalWindow(
          "w-sikemux-watch",
          pane("t-watch", SIKEMUX, "terminal", "vitest --watch"),
          "t-watch",
        ),
        terminalWindow(
          "w-sikemux-logs",
          pane("t-logs", SIKEMUX, "terminal", "tail -f core.log"),
          "t-logs",
        ),
        {
          ...terminalWindow(
            "w-sikemux-gpu",
            {
              type: "pane",
              id: "t-ssh-gpu",
              cwd: SIKEMUX,
              kind: "terminal",
              title: "gpu-box",
              startup: "ssh gpu-box",
            },
            "t-ssh-gpu",
          ),
          customName: "ssh gpu-box",
        },
        {
          id: "w-sikemux-files",
          name: "editor",
          role: "files",
          root: pane("p-editor", SIKEMUX, "editor", "Editor"),
          activePaneId: "p-editor",
        },
        {
          id: "w-sikemux-git",
          name: "Git",
          role: "git",
          root: pane("p-git", SIKEMUX, "git", "Git"),
          activePaneId: "p-git",
        },
        agentWindow(AGENTS.rail),
        agentWindow(AGENTS.replay),
        agentWindow(AGENTS.phone),
      ],
      "s-front": [
        terminalWindow(
          "w-front-term",
          pane("t-front", FRONT, "terminal", "dev"),
          "t-front",
        ),
        agentWindow(AGENTS.empty),
      ],
      "s-mood": [
        terminalWindow(
          "w-mood-term",
          pane("t-mood", MOODBOARD, "terminal", "bench"),
          "t-mood",
        ),
        agentWindow(AGENTS.hero),
        agentWindow(AGENTS.palette),
        agentWindow(AGENTS.notes),
      ],
      "s-billing": [
        terminalWindow(
          "w-billing-service",
          pane(
            "t-billing-service",
            `${DEMO_HOME}/work/billing-service`,
            "terminal",
            "zsh",
          ),
          "t-billing-service",
        ),
        agentWindow(AGENTS.incident),
        agentWindow(AGENTS.flow),
        agentWindow(AGENTS.latency),
      ],
      "s-gpu": [
        terminalWindow(
          "w-gpu",
          {
            type: "pane",
            id: "t-gpu",
            cwd: "",
            kind: "terminal",
            title: "gpu-box",
            startup: "ssh gpu-box",
          },
          "t-gpu",
        ),
      ],
      "s-bastion": [
        terminalWindow(
          "w-bastion",
          {
            type: "pane",
            id: "t-bastion",
            cwd: "",
            kind: "terminal",
            title: "staging-bastion",
            startup: "ssh staging-bastion",
          },
          "t-bastion",
        ),
      ],
      "s-runner": [
        terminalWindow(
          "w-runner",
          {
            type: "pane",
            id: "t-build-runner",
            cwd: "",
            kind: "terminal",
            title: "build-runner",
            startup: "ssh build-runner",
          },
          "t-build-runner",
        ),
      ],
      "s-replica": [
        terminalWindow(
          "w-replica",
          {
            type: "pane",
            id: "t-db-replica",
            cwd: "",
            kind: "terminal",
            title: "db-replica",
            startup: "ssh db-replica",
          },
          "t-db-replica",
        ),
      ],
      "s-pihole": [
        terminalWindow(
          "w-pihole",
          {
            type: "pane",
            id: "t-pi-hole",
            cwd: "",
            kind: "terminal",
            title: "pi-hole",
            startup: "ssh pi-hole",
          },
          "t-pi-hole",
        ),
      ],
      "s-shell": [
        terminalWindow(
          "w-shell",
          pane("t-shell", DEMO_HOME, "terminal", "zsh"),
          "t-shell",
        ),
      ],
      "s-rundeck": [
        {
          id: "w-rundeck",
          name: "Rundeck",
          role: "sikemux.rundeck:deploy",
          fixed: true,
          root: {
            type: "pane",
            id: "p-rundeck",
            cwd: "",
            kind: "sikemux.rundeck:deploy",
            title: "Rundeck",
          },
          activePaneId: "p-rundeck",
        },
      ],
      "s-database": [
        {
          id: "w-database",
          name: "Database",
          role: "sikemux.database:browser",
          fixed: true,
          root: {
            type: "pane",
            id: "p-database",
            cwd: "",
            kind: "sikemux.database:browser",
            title: "Database",
          },
          activePaneId: "p-database",
        },
      ],
      "s-jira": [
        {
          id: "w-jira",
          name: "Jira",
          role: "sikemux.jira:issues",
          fixed: true,
          root: {
            type: "pane",
            id: "p-jira",
            cwd: "",
            kind: "sikemux.jira:issues",
            title: "Jira",
          },
          activePaneId: "p-jira",
        },
      ],
      "s-signoz": [
        {
          id: "w-signoz",
          name: "SigNoz",
          role: "sikemux.signoz:explore",
          fixed: true,
          root: {
            type: "pane",
            id: "p-signoz",
            cwd: "",
            kind: "sikemux.signoz:explore",
            title: "SigNoz",
          },
          activePaneId: "p-signoz",
        },
      ],
    },
    sessionOrder: [
      "s-sikemux",
      "s-front",
      "s-mood",
      "s-billing",
      "s-gpu",
      "s-bastion",
      "s-shell",
      "s-rundeck",
      "s-signoz",
      "s-database",
      "s-jira",
    ],
    activeSessionId: "s-sikemux",
    recent: [],
    prefs: {
      sideRailWidth: 258,
      tabRailWidth: 256,
      sideRailOpen: true,
      tabRailOpen: true,
      lastReleaseNotes: null,
      activeSpaceId: null,
    },
    itemStates: {
      "p-editor": {
        itemId: "p-editor",
        kind: "editor",
        version: 2,
        state: { openTabs: EDITOR_TABS, activePath: EDITOR_TABS[0] },
      },
    },
  };
}
