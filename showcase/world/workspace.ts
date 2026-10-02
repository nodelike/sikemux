import type {
  PersistedAgent,
  PersistedSnapshot,
} from "../../src/state/types/persisted";
import type { LayoutNode, Window } from "../../src/state/types";
import { VERSION } from "../../src/state/persist";
import { DEMO_HOME, DEMO_PROJECTS, FRONT, MOODBOARD, SIKEMUX } from "./projects";

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

const agentWindow = (agent: PersistedAgent): Window => ({
  id: `w-${agent.id}`,
  name: agent.title,
  role: "agent",
  root: pane(agent.id, agent.cwd ?? SIKEMUX, "agent", agent.title),
  activePaneId: agent.id,
});

export const AGENTS = {
  rail: {
    id: "agent-rail",
    type: "claude",
    title: "Line up the sidebar labels",
    resumeId: "5f0c8a1e-rail",
    cwd: SIKEMUX,
  },
  replay: {
    id: "agent-replay",
    type: "codex",
    title: "Fix a flaky terminal test",
    resumeId: "codex-replay-0192",
    cwd: SIKEMUX,
  },
  hero: {
    id: "agent-hero",
    type: "claude",
    title: "Fix the download button on phones",
    resumeId: "91d2e7b4-hero",
    cwd: FRONT,
  },
  palette: {
    id: "agent-palette",
    type: "codex",
    title: "Compare two ways to pick colours",
    resumeId: "codex-palette-7731",
    cwd: MOODBOARD,
  },
  notes: {
    id: "agent-notes",
    type: "hermes",
    title: "Write up the colour results",
    resumeId: "hermes-notes-3310",
    cwd: MOODBOARD,
  },
} satisfies Record<string, PersistedAgent>;

const agents: PersistedAgent[] = Object.values(AGENTS).map((agent) => ({
  ...agent,
  permissionMode:
    agent.id === "agent-replay" || agent.id === "agent-hero"
      ? "bypass"
      : "workspace-write",
  keepAlive: true,
}));

export const EDITOR_TABS = [`${SIKEMUX}/src/rail/AgentRail.tsx`];

export function demoSnapshot(): PersistedSnapshot {
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
        id: "s-shell",
        name: "1",
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
                [0.55, 0.45],
              ),
            ],
            [0.52, 0.48],
          ),
          "t-dev",
        ),
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
      ],
      "s-front": [
        terminalWindow(
          "w-front-term",
          pane("t-front", FRONT, "terminal", "dev"),
          "t-front",
        ),
        agentWindow(AGENTS.hero),
      ],
      "s-mood": [
        terminalWindow(
          "w-mood-term",
          pane("t-mood", MOODBOARD, "terminal", "bench"),
          "t-mood",
        ),
        agentWindow(AGENTS.palette),
        agentWindow(AGENTS.notes),
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
    agents,
    sessionOrder: [
      "s-sikemux",
      "s-front",
      "s-mood",
      "s-gpu",
      "s-bastion",
      "s-shell",
      "s-rundeck",
      "s-signoz",
    ],
    activeSessionId: "s-sikemux",
    recent: [],
    prefs: {
      projectRoots: [{ path: `${DEMO_HOME}/code`, depth: 1 }],
      languageServerTrust: Object.fromEntries(
        DEMO_PROJECTS.map((project) => [project.path, true]),
      ),
      themeId: "aura-noir",
      windowOpacity: 0.81,
      paneShader: true,
      sideRailWidth: 258,
      agentRailWidth: 256,
      windowBlur: 0,
      cloudBrowser: "",
      cloudBrowserShortcut: "",
      sideRailOpen: true,
      agentRailOpen: true,
      zenMode: false,
      onboardingComplete: true,
      notificationsIntroduced: true,
      agentNotifications: false,
      lastReleaseNotes: null,
      lastSeenVersion: "0.4.2",
      pluginSettings: {
        "sikemux.rundeck": { activeProject: "platform", activeGroup: null },
        "sikemux.signoz": { minutes: 60, environment: "production" },
        "sikemux.github": { pinned: [] },
      },
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
