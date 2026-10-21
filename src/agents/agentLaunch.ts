import type { AgentPermissionMode, AgentType } from "../state/types";
import { canBypassPermissions, isAgentType, resumesByPath } from "./catalog";

export const MAX_AGENT_MODEL_LENGTH = 256;

const SESSION_TOKEN = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,199}$/;

/** Whether `id` has the shape the agent's own sessions take, so it can never pass for a flag. Some agents also name a session by its transcript's path. */
export function isResumeId(type: AgentType, id: string): boolean {
    if (SESSION_TOKEN.test(id)) return true;
    return resumesByPath(type) && id.startsWith("/") && id.endsWith(".jsonl") && id.length <= 4_096 && ![...id].some((char) => char < " ");
}

export const AGENT_PERMISSION_MODES: readonly AgentPermissionMode[] = ["workspace-write", "bypass"];

export function supportedPermissionModes(type: AgentType): readonly AgentPermissionMode[] {
    return canBypassPermissions(type) ? AGENT_PERMISSION_MODES : ["workspace-write"];
}

export function normalizePermissionMode(type: AgentType, mode: AgentPermissionMode): AgentPermissionMode {
    return supportedPermissionModes(type).includes(mode) ? mode : "workspace-write";
}

/** The permission mode an agent runs in. */
export function effectiveMode(agent: { permissionMode?: AgentPermissionMode }): AgentPermissionMode {
    return agent.permissionMode ?? "workspace-write";
}

export const AGENT_PERMISSION_COPY: Record<AgentPermissionMode, { label: string; detail: string; tone: "balanced" | "danger" }> = {
    "workspace-write": {
        label: "Normal",
        detail: "Use the provider's normal approval and sandbox behavior.",
        tone: "balanced",
    },
    bypass: {
        label: "YOLO",
        detail: "Bypass approvals and sandboxing.",
        tone: "danger",
    },
};

/** Whether a mode lets the agent act without asking. */
export const isYolo = (mode: string | null | undefined): boolean => mode === "bypass";

/* The core switches a running chat between safe and YOLO for every agent that
   has a YOLO mode: in the CLI's own modes where it has them, or else by
   answering the agent's permission requests itself. */
export const canSwitchMode = (type: string): boolean => isAgentType(type) && canBypassPermissions(type);
