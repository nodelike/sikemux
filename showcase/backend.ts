import type {
  IpcEventListener,
  IpcTransport,
  IpcUnsubscribe,
} from "../src/api/transport";
import type { GitOverview } from "../src/api/git";
import {
  AGENT_SCRIPTS,
  AGENT_USAGE,
  AVAILABLE_AGENTS,
  MODEL_OPTIONS,
  SAVED_SESSIONS,
} from "./world/agents";
import { BRANCHES, GIT_STATUS } from "./world/git";
import { RUNDECK, rundeckStream } from "./world/rundeck";
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
import { DEMO_HOME, DEMO_PROJECTS } from "./world/projects";
import { terminalReplay } from "./world/terminals";
import { demoSnapshot } from "./world/workspace";

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

export class ShowcaseBackend implements IpcTransport {
  readonly unhandled = new Map<string, number>();
  private readonly listeners = new Map<
    string,
    Set<IpcEventListener<unknown>>
  >();
  private readonly handlers = new Map<string, Handler>();
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
        ["rundeck", "signoz", "aws", "bruno"].map((name) => ({
          id: `sikemux.${name}`,
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
    this.on("harness_claim", constant([]));
    this.on("git_worktree_list", constant([]));
    this.on("agent_sessions_watch_start", constant(1));
    this.on("lsp_locations", constant([]));
    this.on("lsp_document_symbols", constant([]));
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
      "pty_ack",
      "pty_unsubscribe",
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
    this.on("git_file_diff", constant([]));
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
        log: await server("git_log", { project: project.name, count: 80 }),
      };
      return overview;
    });
    this.on("git_status", ({ repo }) => GIT_STATUS[repo as string]);
    this.on(
      "git_remotes",
      constant([
        { name: "origin", url: "git@github.com:nodelike/sikemux.git" },
      ]),
    );
    this.on("git_stash_list", constant([]));

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
    this.on("pty_attach", ({ id }) => {
      const pane = ptyPanes.get(id as number);
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

    this.on("available_agents", constant(AVAILABLE_AGENTS));
    this.on(
      "agent_sessions",
      ({ agent }) => SAVED_SESSIONS[agent as string] ?? [],
    );
    this.on(
      "agent_usage",
      ({ agent }) => AGENT_USAGE[agent as keyof typeof AGENT_USAGE] ?? null,
    );
    this.on("acp_start", ({ agentId, provider }) =>
      this.startAgent(agentId as string, provider as string),
    );

    const plugins: Record<string, Record<string, (params: Args) => unknown>> = {
      "sikemux.rundeck": RUNDECK,
      "sikemux.signoz": SIGNOZ,
      "sikemux.aws": AWS,
      "sikemux.bruno": { send: () => CHECKOUT_RESPONSE },
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
          ...script.history,
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
