import { useSyncExternalStore } from "react";
import { invokeCommand as invoke } from "../api/invoke";
import { getIpcTransport } from "../api/transport";
import * as cmd from "../state/commands";
import { agentWindowId, ownerSessionId } from "../state/selectors";
import { getState } from "../state/store";
import { swallow } from "../state/toast";
import type { PtyContext, PtyDirectCommand } from "../state/types";
import { noteSpawnedSession, offerResumableSessions } from "../terminal/sessionResume";
import { agentCwd, agentPtyContext } from "./agentPtyContext";
import { AGENT_NAMES } from "./agentLaunch";
import { afterAgentExit, describeAgentExit, type AgentProcessExit } from "./tuiRecovery";

export type TuiRecovery = { phase: "resuming" } | { phase: "failed"; detail: string | null };

export interface TuiResumeView {
    readonly recovery: TuiRecovery | null;
    /** Moves on each time the agent's terminal is replaced, so its pane takes up the new one. */
    readonly generation: number;
}

export interface TuiSpawnRequest {
    readonly cols: number;
    readonly rows: number;
    readonly cwd: string | null;
    readonly startup: null;
    readonly directCommand: PtyDirectCommand;
    readonly context: PtyContext;
    readonly continues: { session: number; note: string } | null;
}

export interface TuiResumeDeps {
    spawn(request: TuiSpawnRequest): Promise<number>;
    now(): number;
}

interface AgentRecord extends TuiResumeView {
    readonly lastResumeAt: number | null;
}

const IDLE: TuiResumeView = Object.freeze({ recovery: null, generation: 0 });
const records = new Map<string, AgentRecord>();
const listeners = new Set<() => void>();

const defaultDeps: TuiResumeDeps = {
    spawn: (request) => invoke<number>("pty_spawn", { ...request }),
    now: () => Date.now(),
};

function update(agentId: string, change: Partial<AgentRecord>): void {
    const current = records.get(agentId) ?? { ...IDLE, lastResumeAt: null };
    records.set(agentId, { ...current, ...change });
    for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

export function tuiResumeView(agentId: string): TuiResumeView {
    return records.get(agentId) ?? IDLE;
}

export function useTuiResume(agentId: string): TuiResumeView {
    return useSyncExternalStore(
        subscribe,
        () => tuiResumeView(agentId),
        () => IDLE,
    );
}

export function clearTuiRecovery(agentId: string): void {
    if (tuiResumeView(agentId).recovery !== null) update(agentId, { recovery: null });
}

export function resumeNote(type: keyof typeof AGENT_NAMES): string {
    return `— Resuming ${AGENT_NAMES[type]} —`;
}

/**
 * Starts the agent again on its saved conversation as a new terminal that
 * carries the old one's screen, if the core still has it, and points the
 * agent's pane at it.
 */
export async function relaunchTuiAgent(agentId: string, previousPtyId: number | undefined, deps: TuiResumeDeps = defaultDeps): Promise<void> {
    const state = getState();
    const agent = state.agents[agentId];
    const windowId = agentWindowId(state, agentId);
    const sessionId = windowId ? ownerSessionId(state, windowId) : null;
    const session = sessionId ? state.sessions[sessionId] : undefined;
    if (!agent?.directCommand || !session) return;
    update(agentId, { recovery: { phase: "resuming" }, lastResumeAt: deps.now() });
    let id: number;
    try {
        id = await deps.spawn({
            cols: 80,
            rows: 24,
            cwd: agentCwd(agent, session) ?? null,
            startup: null,
            directCommand: agent.directCommand,
            context: agentPtyContext(agent, session),
            continues: previousPtyId === undefined ? null : { session: previousPtyId, note: resumeNote(agent.type) },
        });
    } catch (error) {
        update(agentId, { recovery: { phase: "failed", detail: error instanceof Error ? error.message : String(error) } });
        return;
    }
    noteSpawnedSession(id);
    offerResumableSessions([id]);
    cmd.setAgentPty(agentId, id);
    update(agentId, { recovery: null, generation: tuiResumeView(agentId).generation + 1 });
}

/** Decides what an ended terminal means for the agent running in it, if any. */
export async function noteTerminalExit(ptyId: number, exit: AgentProcessExit, deps: TuiResumeDeps = defaultDeps): Promise<void> {
    const agent = Object.values(getState().agents).find((candidate) => candidate.ptyId === ptyId);
    if (!agent || agent.launchState === "dormant") return;
    const next = afterAgentExit({ exit, resumeId: agent.resumeId, lastResumeAt: records.get(agent.id)?.lastResumeAt ?? null, now: deps.now() });
    if (next === "resume") await relaunchTuiAgent(agent.id, ptyId, deps);
    else if (next === "give-up") update(agent.id, { recovery: { phase: "failed", detail: describeAgentExit(exit) } });
}

interface PtyExitedPayload extends AgentProcessExit {
    readonly id: number;
}

/** Listens for every terminal that ends, shown or not, for as long as the page runs. */
export function watchTerminalAgentExits(deps: TuiResumeDeps = defaultDeps): () => void {
    const controller = new AbortController();
    void getIpcTransport()
        .subscribe<PtyExitedPayload>(
            "pty_exited",
            (event) => {
                const { id, code, signal, killed } = event.payload;
                void noteTerminalExit(id, { code, signal, killed }, deps).catch(swallow("resume terminal agent"));
            },
            { signal: controller.signal },
        )
        .catch((error: unknown) => {
            if (!controller.signal.aborted) swallow("terminal exit listener")(error);
        });
    return () => controller.abort();
}

export function resetTuiResumeForTests(): void {
    records.clear();
    listeners.clear();
}
