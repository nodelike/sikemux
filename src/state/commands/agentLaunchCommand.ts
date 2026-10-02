import { agentLaunchArgs } from "../../agents/agentLaunch";
import { IS_WINDOWS } from "../../lib/platform";
import type { AgentEffort, AgentPermissionMode, AgentType, PtyDirectCommand } from "../types";

type LaunchOptions = { model?: string; effort?: AgentEffort; configPath?: string; environmentKeys?: string[] };

function shellQuote(value: string): string {
    if (/^[A-Za-z0-9_./:=@+-]+$/.test(value)) return value;
    if (IS_WINDOWS) return `'${value.replace(/'/g, "''")}'`;
    return `'${value.replace(/'/g, "'\\''")}'`;
}

export function agentDirectCommand(
    type: AgentType,
    resumeId?: string,
    permissionModeOrSkip: AgentPermissionMode | boolean = "workspace-write",
    executablePath?: string,
    options: LaunchOptions = {},
): PtyDirectCommand {
    const permissionMode: AgentPermissionMode =
        typeof permissionModeOrSkip === "boolean" ? (permissionModeOrSkip ? "bypass" : "workspace-write") : permissionModeOrSkip;
    const profile =
        options.configPath || options.environmentKeys?.length
            ? { configPath: options.configPath, environmentKeys: options.environmentKeys }
            : undefined;
    return {
        program: executablePath?.trim() || type,
        // Prompts are deliberately absent. They are delivered to a direct
        // child PTY after spawn, never exposed in argv or interpreted by a shell.
        args: agentLaunchArgs(type, { resumeId, permissionMode, model: options.model, effort: options.effort }),
        ...(profile ? { profile } : {}),
    };
}

/** Human-readable fallback retained for snapshots and dormant tab summaries. */
export function agentStartup(
    type: AgentType,
    resumeId?: string,
    permissionModeOrSkip: AgentPermissionMode | boolean = "workspace-write",
    executablePath?: string,
    options: LaunchOptions = {},
): string {
    const launch = agentDirectCommand(type, resumeId, permissionModeOrSkip, executablePath, options);
    const invocation = [shellQuote(launch.program), ...launch.args.map(shellQuote)].join(" ");
    return IS_WINDOWS ? `& ${invocation}` : invocation;
}
