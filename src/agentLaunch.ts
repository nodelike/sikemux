import type { AgentEffort, AgentPermissionMode, AgentType } from "./state/types";

export interface AgentLaunchOptions {
    resumeId?: string;
    permissionMode?: AgentPermissionMode;
    model?: string;
    effort?: AgentEffort;
}

export const MAX_AGENT_MODEL_LENGTH = 256;

const AGENT_EFFORTS: Readonly<Record<AgentType, readonly AgentEffort[]>> = {
    claude: ["low", "medium", "high", "xhigh", "max"],
    codex: ["minimal", "low", "medium", "high", "xhigh", "max"],
    hermes: ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"],
    pi: ["off", "minimal", "low", "medium", "high", "xhigh", "max"],
    // The interactive OpenCode command has --model and --prompt, but its
    // provider-specific --variant effort flag belongs to `opencode run`.
    opencode: [],
    omp: ["off", "minimal", "low", "medium", "high", "xhigh", "max"],
    grok: ["none", "minimal", "low", "medium", "high", "xhigh", "max"],
};

export const AGENT_PERMISSION_MODES: readonly AgentPermissionMode[] = ["workspace-write", "bypass"];

export function supportedPermissionModes(type: AgentType): readonly AgentPermissionMode[] {
    return type === "claude" || type === "codex" || type === "hermes" || type === "omp" || type === "grok"
        ? AGENT_PERMISSION_MODES
        : ["workspace-write"];
}

export function normalizePermissionMode(type: AgentType, mode: AgentPermissionMode): AgentPermissionMode {
    return supportedPermissionModes(type).includes(mode) ? mode : "workspace-write";
}

export const AGENT_PERMISSION_COPY: Record<AgentPermissionMode, { label: string; detail: string; tone: "safe" | "balanced" | "open" | "danger" }> = {
    "read-only": {
        label: "Observe",
        detail: "Inspect and plan. Changes require a relaunch with a wider boundary.",
        tone: "safe",
    },
    "workspace-write": {
        label: "Normal",
        detail: "Use the provider's normal approval and sandbox behavior.",
        tone: "balanced",
    },
    "full-access": {
        label: "Operate",
        detail: "Use the whole machine, with the provider's approval flow intact.",
        tone: "open",
    },
    bypass: {
        label: "YOLO",
        detail: "Bypass approvals and sandboxing.",
        tone: "danger",
    },
};

export function supportedEfforts(type: AgentType): readonly AgentEffort[] {
    return AGENT_EFFORTS[type];
}

/** Convert the portable effort choice to the closest level the local CLI accepts. */
export function normalizeAgentEffort(type: AgentType, effort?: AgentEffort): AgentEffort | undefined {
    if (!effort) return undefined;
    const supported = supportedEfforts(type);
    if (supported.includes(effort)) return effort;
    if (effort === "ultra" && supported.includes("max")) return "max";
    return undefined;
}

/**
 * Build raw provider argv. Callers remain responsible for shell quoting each
 * token when converting the result into Sikemux's terminal startup string.
 */
export function agentLaunchArgs(type: AgentType, options: AgentLaunchOptions = {}): string[] {
    const model = options.model?.trim();
    const effort = normalizeAgentEffort(type, options.effort);
    const providerArgs: string[] = [];

    if (model) providerArgs.push("--model", model);
    if (effort) {
        if (type === "claude") providerArgs.push("--effort", effort);
        else if (type === "codex") providerArgs.push("--config", `model_reasoning_effort="${effort}"`);
        else if (type === "hermes") providerArgs.push("--reasoning", effort);
        else if (type === "pi") providerArgs.push("--thinking", effort);
        else if (type === "omp") providerArgs.push("--thinking", effort);
        else if (type === "grok") providerArgs.push("--reasoning-effort", effort);
    }
    if (options.permissionMode) providerArgs.push(...permissionArgs(type, options.permissionMode));

    if (type === "codex" && options.resumeId) {
        return ["resume", ...providerArgs, options.resumeId];
    }

    const resumeArgs = options.resumeId ? resumeArgsForType(type, options.resumeId) : [];
    if (type === "hermes" && (model || effort)) providerArgs.push("--tui");
    return [...providerArgs, ...resumeArgs];
}

function resumeArgsForType(type: AgentType, id: string): string[] {
    if (type === "claude" || type === "hermes" || type === "omp" || type === "grok") return ["--resume", id];
    if (type === "pi" || type === "opencode") return ["--session", id];
    return ["resume", id];
}

/** Provider CLI arguments for Sikemux's Normal and YOLO modes. */
export function permissionArgs(type: AgentType, mode: AgentPermissionMode): string[] {
    mode = normalizePermissionMode(type, mode);
    if (type === "codex") {
        if (mode === "bypass") return ["--dangerously-bypass-approvals-and-sandbox"];
        return ["--sandbox", mode === "full-access" ? "danger-full-access" : mode];
    }
    if (type === "claude") {
        if (mode === "bypass") return ["--dangerously-skip-permissions"];
        if (mode === "read-only") return ["--permission-mode", "plan"];
        if (mode === "workspace-write") return ["--permission-mode", "acceptEdits"];
        return ["--permission-mode", "default"];
    }
    if (type === "hermes" && mode === "bypass") return ["--yolo"];
    if (type === "omp" && mode === "bypass") return ["--approval-mode", "yolo"];
    if (type === "grok" && mode === "bypass") return ["--permission-mode", "bypassPermissions"];
    return [];
}

export function isDangerousPermissionMode(mode: AgentPermissionMode): boolean {
    return mode === "bypass";
}
