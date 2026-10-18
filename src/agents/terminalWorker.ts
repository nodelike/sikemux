import { coreSessionsApi } from "../api/coreSessions";
import { invokeCommand as invoke } from "../api/invoke";
import * as cmd from "../state/commands";
import type { TerminalWorker } from "../state/commands";
import { noteSpawnedSession, offerResumableSessions } from "../terminal/sessionResume";
import { agentPtyContext } from "./agentPtyContext";

function text(params: Record<string, unknown>, key: string): string {
    const value = params[key];
    if (typeof value !== "string" || !value) throw new Error(`${key} must be text`);
    return value;
}

function optionalText(params: Record<string, unknown>, key: string): string | null {
    const value = params[key];
    if (value === undefined || value === null) return null;
    if (typeof value !== "string") throw new Error(`${key} must be text`);
    return value;
}

function terminalWorker(params: Record<string, unknown>): TerminalWorker {
    return {
        agentId: text(params, "agentId"),
        cli: text(params, "cli"),
        name: text(params, "name"),
        lead: text(params, "lead"),
        model: optionalText(params, "model"),
        permissionMode: text(params, "permissionMode"),
        launcher: optionalText(params, "launcher"),
    };
}

/**
 * Starts an agent another agent asked for in a terminal and shows it in a tab beside its lead, without taking the
 * person there. The terminal runs from the start, so its task can be typed in before anyone looks at it.
 */
export async function openTerminalWorker(params: Record<string, unknown>): Promise<{ ptyId: number }> {
    const plan = cmd.planTerminalWorker(terminalWorker(params));
    if (!plan?.agent.directCommand) throw new Error("the agent that asked for it is no longer open in Sikemux");
    const { agent, session } = plan;
    const ptyId = await invoke<number>("pty_spawn", {
        cols: 100,
        rows: 30,
        cwd: agent.cwd ?? null,
        startup: null,
        directCommand: agent.directCommand,
        context: agentPtyContext(agent, session),
        continues: null,
    });
    noteSpawnedSession(ptyId);
    offerResumableSessions([ptyId]);
    if (!cmd.adoptTerminalWorker({ ...agent, ptyId }, session.id)) {
        void coreSessionsApi.kill(ptyId).catch(() => {});
        throw new Error("its project closed while its terminal started");
    }
    return { ptyId };
}
