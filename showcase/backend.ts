import { DATABASE } from "./world/database";
import { SIKEMUX as SIKEMUX_PATH } from "./world/projects";
import type {
  IpcEventListener,
  IpcTransport,
  IpcUnsubscribe,
} from "../src/api/transport";
import type { CommitOwner, GitCommit, GitOverview } from "../src/api/git";
import {
  AGENT_SCRIPTS,
  AGENT_USAGE,
  AVAILABLE_AGENTS,
  MODEL_OPTIONS,
  SAVED_SESSIONS,
} from "./world/agents";
import { BRANCHES, FILE_DIFFS, GIT_STATUS } from "./world/git";
import { RUNDECK, rundeckStream } from "./world/rundeck";
import { GITHUB } from "./world/github";
import { JIRA } from "./world/jira";
import { VERCEL } from "./world/vercel";
import { SIGNOZ, signozTail } from "./world/signoz";
import { AWS, awsLogLines } from "./world/aws";
import { demoActivity } from "./world/activity";
import {
  BRUNO_COLLECTION,
  BRUNO_FILES,
  CHECKOUT_RESPONSE,
  brunoDir,
} from "./world/bruno";
import { BROWSER_TABS, placeBrowserPage } from "./browserPage";
import {
  DEMO_HOME,
  DEMO_PROJECTS,
  PANE_IMAGE,
  SIKEMUX,
} from "./world/projects";
import { terminalReplay } from "./world/terminals";
import { PtyOutput, type PtyChannel } from "./ptyOutput";
import { AGENTS, demoSnapshot } from "./world/workspace";
import { DEMO_BOARD, DEMO_MESSAGES } from "./world/board";
import type { BoardCard } from "../src/agents/agentBoard";

type Args = Record<string, unknown>;
type Handler = (args: Args) => unknown;

async function server<T>(endpoint: string, input: unknown): Promise<T> {
  const response = await fetch(`/__showcase/${endpoint}`, {
    method: "POST",
    body: JSON.stringify(input),
  });
  const value = await response.json();
  if (!response.ok) throw new Error(value.error);
  return value as T;
}

interface RecentRequest {
  providers: { agent: string }[];
  projects: string[];
  limit: number;
  cursor?: { key: string } | null;
  query?: string;
  exclude: { agent: string; id: string }[];
}

function recentPage(request: RecentRequest) {
  const rows = request.providers
    .flatMap(({ agent }) =>
      (SAVED_SESSIONS[agent] ?? []).map((row) => ({ ...row, agent })),
    )
    .filter((row) => request.projects.includes(row.project))
    .filter(
      (row) =>
        !request.exclude.some(
          (open) => open.agent === row.agent && open.id === row.id,
        ),
    )
    .filter(
      (row) =>
        !request.query || row.title.toLowerCase().includes(request.query),
    )
    .sort((a, b) => b.mtime - a.mtime);
  const start = request.cursor ? Number(request.cursor.key) : 0;
  const sessions = rows.slice(start, start + request.limit);
  const end = start + sessions.length;
  return {
    sessions,
    next: end < rows.length ? { atMs: 0, agent: "", key: String(end) } : null,
  };
}

/** The core's session search, by title anywhere and by words that start a word in what was said. */
function sessionSearch({
  query,
  limit = 8,
}: {
  query: string;
  limit?: number;
}) {
  const needle = query.trim().toLowerCase();
  const words = needle
    .split(/[^\p{L}\p{N}]+/u)
    .filter((word) => word.length > 1);
  const startsWord = (text: string, word: string) =>
    new RegExp(
      `(^|[^\\p{L}\\p{N}])${word.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`,
      "u",
    ).test(text);
  return Object.entries(SAVED_SESSIONS)
    .flatMap(([cli, rows]) => rows.map((row) => ({ ...row, cli })))
    .map((row) => {
      const title = row.title.toLowerCase();
      const said = (row.said ?? "").toLowerCase();
      const inTitle = title.includes(needle)
        ? startsWord(title, needle)
          ? 2
          : 1
        : 0;
      const inSaid = words.filter((word) => startsWord(said, word)).length;
      return { row, rank: inTitle * 10 + inSaid };
    })
    .filter(({ rank }) => rank > 0)
    .sort((a, b) => b.rank - a.rank || b.row.mtime - a.row.mtime)
    .slice(0, limit)
    .map(({ row }) => ({
      cli: row.cli,
      sessionId: row.id,
      title: row.title,
      project: row.project,
      activeAt: row.mtime * 1000,
      snippets: row.said ? [row.said] : [],
    }));
}

const SIM_DEVICE = {
  udid: "5C1A0E77-8D2F-4B1E-9F63-2A7D41C0B9E8",
  name: "iPhone 18 Pro",
  state: "booted",
  runtime: "iOS 27.0",
  model: "iPhone 18 Pro",
};
const SIM_ATTACHED = {
  agentId: "agent-phone",
  udid: SIM_DEVICE.udid,
  name: SIM_DEVICE.name,
  project: SIKEMUX_PATH,
};

const OTHER_ACCOUNTS: Record<string, { email: string; used: number }> = {
  ".claude-personal": { email: "edon@hey.com", used: 82 },
  ".claude-client": { email: "edon@northwind.io", used: 9 },
};

/* A real chat's history says when each part of it happened, which is how a
   finished turn knows how long it worked. The demo's arrive nine seconds apart. */
function recorded(
  history: Record<string, unknown>[],
): Record<string, unknown>[] {
  const start = Date.now() - history.length * 9_000;
  return history.map((update, index) => ({
    ...update,
    _meta: { sikemux: { at: start + index * 9_000 } },
  }));
}

/** Commits agents made, by their place in the history, with the agents their signed footers name. */
const AGENT_COMMITS: Record<number, CommitOwner[]> = {
  0: [
    {
      name: AGENTS.rail.name,
      cli: AGENTS.rail.type,
      model: "opus",
      chat: AGENTS.rail.id,
      title: AGENTS.rail.title,
      open: AGENTS.rail.id,
      sessionId: AGENTS.rail.resumeId,
    },
  ],
  2: [
    {
      name: AGENTS.replay.name,
      cli: AGENTS.replay.type,
      chat: AGENTS.replay.id,
      title: AGENTS.replay.title,
      open: AGENTS.replay.id,
      sessionId: AGENTS.replay.resumeId,
    },
    {
      name: "Sable",
      cli: "claude",
      chat: "agent-sable",
      title: "Release notes for v0.4.1",
      sessionId: "s-c3",
    },
  ],
  5: [
    {
      name: "Juniper",
      cli: "claude",
      chat: "agent-juniper",
      title: "Split the Rundeck matrix into its own pane",
      sessionId: "s-c1",
    },
  ],
};
const commitOwners = new Map<string, CommitOwner[]>();

function signedByAgents(log: GitCommit[]): GitCommit[] {
  return log.map((commit, index) => {
    const owners = AGENT_COMMITS[index];
    if (!owners) return commit;
    commitOwners.set(commit.full_hash, owners);
    return { ...commit, footer: true };
  });
}

export class ShowcaseBackend implements IpcTransport {
  readonly unhandled = new Map<string, number>();
  private readonly listeners = new Map<
    string,
    Set<IpcEventListener<unknown>>
  >();
  private readonly handlers = new Map<string, Handler>();
  private readonly outputs = new Map<string, PtyOutput>();
  private sequence = 0;

  constructor() {
    this.registerAll();
  }

  async invoke<Result>(command: string, args?: unknown): Promise<Result> {
    const handler = this.handlers.get(command);
    if (!handler) {
      this.unhandled.set(command, (this.unhandled.get(command) ?? 0) + 1);
      return null as Result;
    }
    return (await handler((args ?? {}) as Args)) as Result;
  }

  async subscribe<Payload>(
    event: string,
    listener: IpcEventListener<Payload>,
  ): Promise<IpcUnsubscribe> {
    const set = this.listeners.get(event) ?? new Set();
    set.add(listener as IpcEventListener<unknown>);
    this.listeners.set(event, set);
    return () => set.delete(listener as IpcEventListener<unknown>);
  }

  emit(event: string, payload: unknown): void {
    for (const listener of this.listeners.get(event) ?? [])
      listener({ event, id: ++this.sequence, payload });
  }

  on(command: string, handler: Handler): void {
    this.handlers.set(command, handler);
  }

  terminalOutput(paneId: string): PtyOutput | undefined {
    return this.outputs.get(paneId);
  }

  private registerAll(): void {
    const constant = (value: unknown) => () => value;

    this.on(
      "boot_init",
      constant({
        home: DEMO_HOME,
        state: JSON.stringify(demoSnapshot()),
        recent: [],
      }),
    );
    this.on(
      "plugin_manifests",
      constant(
        [
          ["rundeck", "Rundeck"],
          ["signoz", "SigNoz"],
          ["aws", "AWS"],
          ["bruno", "Bruno"],
          ["github", "GitHub"],
          ["database", "Database"],
          ["vercel", "Vercel"],
        ].map(([id, name]) => ({
          id: `sikemux.${id}`,
          name,
          version: "1.0.0",
          sikemux: "^0.4.0",
        })),
      ),
    );
    this.on(
      "battery_status",
      constant({ percent: 86, charging: false, time_remaining: null }),
    );
    this.on("cli_frontend_ready", constant([]));
    this.on("take_deep_links", constant([]));
    this.on("harness_claim", constant([]));
    this.on("git_worktree_list", constant([]));
    this.on("agent_sessions_watch_start", constant(1));
    this.on("lsp_locations", constant([]));
    this.on("lsp_document_symbols", constant([]));
    this.on("simulator_attachments", constant([SIM_ATTACHED]));
    this.on(
      "sim_status",
      constant({ supported: true, installed: true, reason: null }),
    );
    this.on("simulator_orientation", constant("portrait"));
    this.on("sim_call", async ({ request }) => {
      const { type } = request as { type: string };
      if (type === "devices") return { devices: [SIM_DEVICE] };
      if (type === "screen")
        return { width: 402, height: 874, scale: 3, orientation: "portrait" };
      if (type === "chrome" || type === "mask") {
        const art = (await (
          await fetch("/__showcase/simulator/chrome")
        ).json()) as Record<string, unknown>;
        return { [type]: art[type] ?? null };
      }
      return null;
    });
    this.on("sim_watch", async ({ format, onFrame }) => {
      if (format !== "mjpeg")
        throw { category: "unavailable", message: "streamUnavailable" };
      const frame = await (
        await fetch("/__showcase/simulator/frame")
      ).arrayBuffer();
      const channel = onFrame as { onmessage: (frame: ArrayBuffer) => void };
      for (const delay of [50, 400, 1200])
        setTimeout(() => channel.onmessage(frame.slice(0)), delay);
      return 1;
    });
    this.on("activity_summary", demoActivity);
    for (const quiet of [
      "observability_ui_heartbeat",
      "observability_ui_activity",
      "plugin_set_disabled",
      "set_window_blur",
      "state_save",
      "repo_watch_start",
      "repo_watch_stop",
      "path_kinds",
      "activity_turn_started",
      "activity_turn_ended",
      "write_file",
      "lsp_start",
      "lsp_open",
      "lsp_change",
      "lsp_change_incremental",
      "lsp_close",
      "lsp_stop",
      "pty_resize",
      "pty_write",
      "pty_unsubscribe",
      "sim_watch_read",
      "sim_unwatch",
      "sim_prepare",
      "simulator_set_enabled",
    ]) {
      this.on(quiet, constant(null));
    }
    this.on(
      "ssh_hosts",
      constant([
        {
          alias: "staging-bastion",
          hostname: "10.20.0.4",
          user: "edon",
          port: 22,
        },
        { alias: "gpu-box", hostname: "gpu.lan", user: "edon", port: 22 },
        { alias: "pi-hole", hostname: "192.168.1.2", user: "pi", port: 22 },
        { alias: "build-runner", hostname: "10.20.0.12", user: "ci", port: 22 },
        { alias: "db-replica", hostname: "10.20.0.31", user: "edon", port: 22 },
      ]),
    );
    this.on(
      "scan_project_roots",
      constant(DEMO_PROJECTS.map(({ name, path }) => ({ name, path }))),
    );

    this.on("read_dirs", ({ paths }) => server("read_dirs", { paths }));
    this.on("markdown_parse", ({ requests }) =>
      server("markdown", { requests }),
    );
    this.on(
      "read_file",
      ({ path }) =>
        BRUNO_FILES[path as string] ?? server("read_file", { path }),
    );
    this.on("read_dir", ({ path }) =>
      (path as string).startsWith(BRUNO_COLLECTION)
        ? brunoDir(path as string)
        : [],
    );
    this.on("preview_file", ({ path }) =>
      path === PANE_IMAGE ? { mime: "image/jpeg", size: 0, modified: 0 } : null,
    );
    this.on("read_file_versioned", async ({ path }) => ({
      content: await server<string>("read_file", { path }),
      version: "showcase",
    }));
    this.on("read_text_file_limited", ({ path }) =>
      server("read_file", { path }),
    );
    this.on("list_project_files_snapshot", async ({ repo }) => ({
      scanId: 1,
      files: await server<string[]>("list_files", { repo }),
    }));
    this.on("git_file_at", async ({ repo, path }) =>
      server("read_file", { path: `${repo}/${path}` }).catch(() => ""),
    );
    this.on("git_file_diff", ({ path }) => FILE_DIFFS[path as string] ?? []);
    this.on("git_blame", constant({ commits: [], lines: [] }));

    this.on("git_overview", async ({ repo }) => {
      const project = DEMO_PROJECTS.find(
        (candidate) => candidate.path === repo,
      );
      const status = GIT_STATUS[repo as string];
      if (!project || !status) throw new Error("could not find repository");
      const overview: GitOverview = {
        status,
        branches: (BRANCHES[project.path] ?? ["main"]).map((name) => ({
          name,
          current: name === status.branch,
          upstream: name === status.branch ? status.upstream : null,
        })),
        log: signedByAgents(
          await server<GitCommit[]>("git_log", {
            project: project.name,
            count: 80,
          }),
        ),
      };
      return overview;
    });
    this.on("git_commit_owners", ({ commits }) =>
      Object.fromEntries(
        (commits as string[]).flatMap((hash) => {
          const owners = commitOwners.get(hash);
          return owners ? [[hash, owners]] : [];
        }),
      ),
    );
    this.on("git_status", ({ repo }) => GIT_STATUS[repo as string]);
    this.on(
      "git_remotes",
      constant([
        { name: "origin", url: "git@github.com:nodelike/sikemux.git" },
      ]),
    );
    this.on("git_stash_list", constant([]));
    this.on("git_commit_files", async ({ repo, rev }) => {
      const project = DEMO_PROJECTS.find(
        (candidate) => candidate.path === repo,
      );
      if (!project) return [];
      return server<string[]>("commit_files", { project: project.name, rev });
    });
    this.on("git_compare", async ({ repo }) => {
      const project = DEMO_PROJECTS.find(
        (candidate) => candidate.path === repo,
      );
      if (!project) throw new Error("could not find repository");
      const log = await server<GitCommit[]>("git_log", {
        project: project.name,
        count: 4,
      });
      const commits = log.slice(0, 3);
      const paths = new Set<string>();
      for (const commit of commits) {
        const files = await server<string[]>("commit_files", {
          project: project.name,
          rev: commit.full_hash,
        });
        files.forEach((path) => paths.add(path));
      }
      return {
        merge_base: log[3]?.full_hash ?? "",
        files: [...paths].map((path) => ({ path, status: "M" })),
        commits,
      };
    });
    this.on("git_remote_branches", ({ repo, remote }) =>
      (BRANCHES[repo as string] ?? ["main"]).map((name) => ({
        name,
        full_ref: `${remote}/${name}`,
        is_head_pointer: false,
        tracked_by: name,
        subject: null,
      })),
    );

    let nextPty = 1;
    const ptyPanes = new Map<number, { paneId?: string; cwd: string | null }>();
    this.on("pty_spawn", ({ cwd, context }) => {
      const id = nextPty++;
      ptyPanes.set(id, {
        paneId: (context as { paneId?: string } | null)?.paneId,
        cwd: cwd as string | null,
      });
      return id;
    });
    this.on("pty_attach", ({ id, onEvent }) => {
      const pane = ptyPanes.get(id as number);
      if (pane?.paneId)
        this.outputs.set(pane.paneId, new PtyOutput(onEvent as PtyChannel));
      const replay = new TextEncoder().encode(
        terminalReplay(pane?.paneId, pane?.cwd ?? null),
      );
      const header = new TextEncoder().encode(
        JSON.stringify({ subId: 1, alternateScreen: false, shell: null }),
      );
      const out = new Uint8Array(4 + header.length + replay.length);
      new DataView(out.buffer).setUint32(0, header.length, true);
      out.set(header, 4);
      out.set(replay, 4 + header.length);
      return out.buffer;
    });
    this.on("pty_ack", ({ id, bytes }) => {
      const paneId = ptyPanes.get(id as number)?.paneId;
      if (paneId) this.outputs.get(paneId)?.ack(bytes as number);
    });

    this.on("available_agents", constant(AVAILABLE_AGENTS));
    this.on("agent_sessions", ({ agent, cwd }) =>
      (SAVED_SESSIONS[agent as string] ?? []).filter(
        (row) => row.project === cwd,
      ),
    );
    this.on("agent_recent_sessions", ({ request }) =>
      recentPage(request as RecentRequest),
    );
    this.on("agent_accounts_call", ({ method, params }) => {
      const { provider, configPath } = (params ?? {}) as Args;
      const account = OTHER_ACCOUNTS[String(configPath).split("/").pop() ?? ""];
      if (method === "agentUsage") {
        const usage = AGENT_USAGE[provider as keyof typeof AGENT_USAGE] ?? null;
        if (!usage || !account) return usage;
        return {
          ...usage,
          windows: usage.windows.map((window, index) => ({
            ...window,
            usedPercent:
              index === 0 ? account.used : Math.round(account.used / 2),
          })),
        };
      }
      if (method === "agentAccountStatus")
        return {
          signedIn: true,
          name: null,
          email: account?.email ?? "edon@acme.dev",
          plan: null,
          organization: null,
          method: "subscription",
          sessions: `/demo/${String(provider)}/sessions`,
        };
      return null;
    });
    this.on("model_providers", () => []);
    this.on("acp_attach", () => ({ status: "missing" }));
    this.on("acp_list", () => []);
    this.on("pty_sessions", () => []);
    this.on("listening_ports", () => []);
    const demoNames = new Map<string, string>(
      Object.values(AGENTS).map((agent) => [agent.id, agent.name]),
    );
    const spareNames = ["Lark", "Fern", "Quartz", "Dune", "Pika", "Aster"];
    this.on("agent_name_call", ({ method, params }) => {
      if (method === "agentsInApp") return null;
      if (method === "agentSessions")
        return sessionSearch(params as { query: string; limit?: number });
      const { agentId, name } = params as { agentId: string; name?: string };
      if (method === "renameAgent" && name) demoNames.set(agentId, name);
      if (!demoNames.has(agentId))
        demoNames.set(
          agentId,
          spareNames[demoNames.size % spareNames.length] ?? "Lark",
        );
      return {
        agentId,
        name: demoNames.get(agentId),
        project: SIKEMUX,
        parent: null,
      };
    });
    this.on("inbox_call", ({ method, params }) => {
      if (method === "agentMessages")
        return {
          project: (params as { project: string }).project,
          entries: DEMO_MESSAGES,
        };
      return {
        items: [],
        working: 0,
        away: false,
        agentMessages: {},
        collisions: {},
        remoteAgents: [],
      };
    });
    let board = DEMO_BOARD;
    this.on("agent_board_call", ({ method, params }) => {
      const { id, status } = (params ?? {}) as {
        id?: number;
        status?: BoardCard["status"];
      };
      const changes = status ? { status } : {};
      if (method === "boards") return [board];
      if (method === "boardUpdate")
        board = {
          ...board,
          cards: board.cards.map((card) =>
            card.id === id ? { ...card, ...changes } : card,
          ) as typeof board.cards,
        };
      if (method === "boardSettings")
        board = {
          ...board,
          settings: { ...board.settings, ...(params as object) },
        };
      if (method === "boardRemove")
        board = {
          ...board,
          cards: board.cards.filter((card) => card.id !== id),
        };
      return board;
    });
    this.on("acp_start", ({ agentId, provider }) =>
      this.startAgent(agentId as string, provider as string),
    );

    const plugins: Record<string, Record<string, (params: Args) => unknown>> = {
      "sikemux.rundeck": RUNDECK,
      "sikemux.signoz": SIGNOZ,
      "sikemux.aws": AWS,
      "sikemux.bruno": { send: () => CHECKOUT_RESPONSE },
      "sikemux.github": GITHUB,
      "sikemux.database": DATABASE,
      "sikemux.jira": JIRA,
      "sikemux.vercel": VERCEL,
    };
    this.on("plugin_call", ({ plugin, method, params }) => {
      const answer = plugins[plugin as string]?.[method as string];
      if (!answer)
        throw {
          category: "unavailable",
          message: `showcase has no ${String(plugin)}.${String(method)}`,
        };
      return answer((params ?? {}) as Args);
    });
    let nextStream = 1;
    this.on("plugin_stream_start", ({ plugin, method, onEvent }) => {
      const channel = onEvent as { onmessage: (event: unknown) => void };
      const emit = (value: unknown) =>
        setTimeout(() => channel.onmessage({ kind: "item", value }), 30);
      if (plugin === "sikemux.rundeck") rundeckStream(method as string, emit);
      if (plugin === "sikemux.aws" && method === "tailLogs")
        for (const line of awsLogLines()) emit(line);
      if (plugin === "sikemux.signoz" && method === "tailLogs")
        emit(signozTail());
      return nextStream++;
    });

    this.on(
      "browser_snapshot",
      ({ agentId }) =>
        BROWSER_TABS[agentId as string] ?? { tabs: [], activeTabId: null },
    );
    this.on("browser_set_bounds", ({ agentId, bounds }) =>
      placeBrowserPage(agentId as string, bounds as DOMRectInit | null),
    );

    const phone =
      "4b1e8c0d27a95f36e1c4b80d9a2f6e7135c0b8d4e9f2a6c1d07e3b5f9a8c2d41";
    const remote = {
      enabled: true,
      coreId:
        "7d3f9c2ae0b54d18a6f1c39e85b27d0c4fa16e93b2d8c05a7e14f69b3c2d8a50",
      addresses: [] as string[],
      devices: [
        {
          id: phone,
          name: "iPhone",
          platform: "ios",
          access: "full",
          pairedAt: Date.now() - 9 * 86_400_000,
          lastSeen: Date.now(),
        },
        {
          id: "9a0c5e3b7d1f48a2c6e09b4d8f3a1c7e5b2d06f9a4c8e1b3d7f5a2c09e6b4d18",
          name: "Pixel 9",
          platform: "android",
          access: "watch",
          pairedAt: Date.now() - 30 * 86_400_000,
          lastSeen: Date.now() - 2 * 3_600_000,
        },
      ],
      connected: [phone],
      pending: [] as {
        id: string;
        deviceId: string;
        name: string;
        platform: string;
      }[],
    };
    this.on("remote_status", () => ({ ...remote }));
    this.on("remote_set_enabled", ({ enabled }) => ({
      ...Object.assign(remote, { enabled: enabled as boolean }),
    }));
  }

  private readonly liveSteps: { run: () => void; holdMs: number }[] = [];

  // A live turn plays one step at a time so the capture can move the clock between tool calls.
  stepLive(): number {
    const step = this.liveSteps.shift();
    if (!step) return -1;
    step.run();
    return step.holdMs;
  }

  private queueLiveTurn(
    agentId: string,
    live: Record<string, unknown>[],
    batch: (updates: Record<string, unknown>[]) => unknown,
  ) {
    const send = (updates: Record<string, unknown>[]) =>
      this.emit("acp_event", {
        agentId,
        kind: "session_update",
        payload: batch(updates),
      });
    const strip = ({
      durationMs: _durationMs,
      ...update
    }: Record<string, unknown>) => update;
    this.liveSteps.push({
      run: () =>
        this.emit("acp_event", { agentId, kind: "turn_started", payload: {} }),
      holdMs: 400,
    });
    let finishing: Record<string, unknown> | null = null;
    for (const update of live) {
      const isTool = update.sessionUpdate === "tool_call";
      const previous = finishing;
      finishing = isTool && update.status === "completed" ? update : null;
      const shown = isTool
        ? { ...strip(update), status: "in_progress" }
        : strip(update);
      this.liveSteps.push({
        run: () =>
          send([
            ...(previous
              ? [
                  {
                    sessionUpdate: "tool_call_update",
                    toolCallId: previous.toolCallId,
                    status: "completed",
                  },
                ]
              : []),
            shown,
          ]),
        holdMs: isTool ? Number(update.durationMs ?? 1200) : 600,
      });
    }
  }

  private startAgent(agentId: string, provider: string) {
    const script = AGENT_SCRIPTS[agentId];
    const setup = { configOptions: MODEL_OPTIONS[provider] ?? [] };
    const sessionId = `session-${agentId}`;
    const batch = (updates: Record<string, unknown>[]) => ({
      updates: updates.map((update) => ({ sessionId, update })),
    });
    if (script) {
      this.emit("acp_event", {
        agentId,
        kind: "session_update",
        payload: batch([
          ...recorded(script.history),
          { sessionUpdate: "usage_update", ...script.usage },
        ]),
      });
    }
    this.emit("acp_event", {
      agentId,
      kind: "ready",
      payload: { capabilities: {}, setup },
    });
    if (script?.live) this.queueLiveTurn(agentId, script.live, batch);
    return { sessionId, capabilities: {}, setup };
  }
}
