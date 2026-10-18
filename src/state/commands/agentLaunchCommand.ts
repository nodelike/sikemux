import { agentLaunchArgs, effectiveMode } from "../../agents/agentLaunch";
import { IS_WINDOWS } from "../../lib/platform";
import type { StoreState } from "../store";
import type { Agent, AgentEffort, AgentPermissionMode, AgentType, ProviderProfile, PtyDirectCommand } from "../types";

type LaunchOptions = { model?: string; effort?: AgentEffort; configPath?: string; environmentKeys?: string[] };

export function shellQuote(value: string): string {
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

/** The account an agent runs under, when it is one of the person's profiles for that provider. */
export function profileOf(state: Pick<StoreState, "providerProfiles">, agent: Pick<Agent, "profileId" | "type">): ProviderProfile | undefined {
    return agent.profileId ? state.providerProfiles.find((profile) => profile.id === agent.profileId && profile.provider === agent.type) : undefined;
}

/** The CLI an agent launches: its profile's, else the one it was started with. */
export function agentExecutablePath(
    state: Pick<StoreState, "providerProfiles">,
    agent: Pick<Agent, "profileId" | "type" | "executablePath">,
): string | undefined {
    return profileOf(state, agent)?.executablePath || agent.executablePath;
}

type LaunchedAgent = Pick<Agent, "type" | "profileId" | "executablePath" | "resumeId" | "permissionMode" | "skipPermissions" | "model" | "effort">;

/** The command that starts an agent as it stands now; set again whenever its session, account, mode or model changes. */
export function relaunchSpec(state: Pick<StoreState, "providerProfiles">, agent: LaunchedAgent): Pick<Agent, "startup" | "directCommand"> {
    const profile = profileOf(state, agent);
    const executable = profile?.executablePath || agent.executablePath;
    const mode = effectiveMode(agent);
    const options = { model: agent.model, effort: agent.effort, configPath: profile?.configPath, environmentKeys: profile?.environmentKeys };
    return {
        startup: agentStartup(agent.type, agent.resumeId, mode, executable, options),
        directCommand: agentDirectCommand(agent.type, agent.resumeId, mode, executable, options),
    };
}
