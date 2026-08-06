import type { AgentType, SessionKind, Window } from "./types";
import { isRecord, validatePersistedWindow, type LayoutValidationLimits } from "./persistValidation";

const SESSION_KINDS = new Set<SessionKind>(["project", "command", "ssh", "aws", "rundeck", "bruno"]);
const AGENT_TYPES = new Set<AgentType>(["claude", "codex", "hermes", "pi", "opencode"]);

export const SESSION_BUNDLE_LIMITS = {
    maxBytes: 1_048_576,
    maxWindows: 64,
    maxAgents: 64,
    maxTotalLayoutNodes: 2_048,
    maxStringLength: 8_192,
    maxDepth: 32,
    maxNodesPerWindow: 1_024,
    maxChildren: 64,
} as const;

export interface ImportedAgent {
    type: AgentType;
    title: string;
    resumeId: string;
}

export interface ValidSessionBundle {
    session: { name: string; cwd: string; kind: SessionKind };
    windows: Window[];
    agents: ImportedAgent[];
}

function requiredString(value: unknown, label: string, allowEmpty = false): string {
    if (typeof value !== "string" || value.length > SESSION_BUNDLE_LIMITS.maxStringLength || (!allowEmpty && value.length === 0))
        throw new Error(`session bundle ${label} is invalid`);
    return value;
}

export function parseSessionBundle(raw: string): ValidSessionBundle {
    if (raw.length > SESSION_BUNDLE_LIMITS.maxBytes) throw new Error("session bundle exceeds the 1 MiB clipboard limit");
    let decoded: unknown;
    try {
        decoded = JSON.parse(raw);
    } catch {
        throw new Error("clipboard does not contain valid JSON");
    }
    if (!isRecord(decoded) || decoded.format !== "sikemux-session") throw new Error("clipboard is not a Sikemux session bundle");
    if (decoded.version !== 1) throw new Error("session bundle version is unsupported");
    if (!isRecord(decoded.session)) throw new Error("session bundle is incomplete");

    const name = requiredString(decoded.session.name, "name");
    const cwd = requiredString(decoded.session.cwd, "cwd", true);
    if (!SESSION_KINDS.has(decoded.session.kind as SessionKind)) throw new Error("session kind is unsupported");
    const kind = decoded.session.kind as SessionKind;

    if (!Array.isArray(decoded.windows) || decoded.windows.length === 0) throw new Error("session bundle contains no windows");
    if (decoded.windows.length > SESSION_BUNDLE_LIMITS.maxWindows)
        throw new Error(`session bundle exceeds the window limit (${SESSION_BUNDLE_LIMITS.maxWindows})`);
    const windowIds = new Set<string>();
    const layoutIds = new Set<string>();
    let totalNodes = 0;
    const layoutLimits: LayoutValidationLimits = {
        maxDepth: SESSION_BUNDLE_LIMITS.maxDepth,
        maxNodes: SESSION_BUNDLE_LIMITS.maxNodesPerWindow,
        maxChildren: SESSION_BUNDLE_LIMITS.maxChildren,
        maxStringLength: SESSION_BUNDLE_LIMITS.maxStringLength,
    };
    const windows = decoded.windows.map((row, index) => {
        const validated = validatePersistedWindow(row, layoutLimits);
        if (!validated) throw new Error(`session bundle window ${index + 1} is invalid or unsupported`);
        if (windowIds.has(validated.window.id)) throw new Error(`duplicate window id: ${validated.window.id}`);
        windowIds.add(validated.window.id);
        for (const id of validated.layout.ids) {
            if (layoutIds.has(id)) throw new Error(`duplicate layout id across windows: ${id}`);
            layoutIds.add(id);
        }
        totalNodes += validated.layout.ids.length;
        if (totalNodes > SESSION_BUNDLE_LIMITS.maxTotalLayoutNodes)
            throw new Error(`session bundle exceeds the total layout node limit (${SESSION_BUNDLE_LIMITS.maxTotalLayoutNodes})`);
        return validated.window;
    });

    if (!Array.isArray(decoded.agents)) throw new Error("session bundle agents are invalid");
    if (decoded.agents.length > SESSION_BUNDLE_LIMITS.maxAgents)
        throw new Error(`session bundle exceeds the agent limit (${SESSION_BUNDLE_LIMITS.maxAgents})`);
    const claims = new Set<string>();
    const agents = decoded.agents.map((row, index): ImportedAgent => {
        if (!isRecord(row) || !AGENT_TYPES.has(row.type as AgentType)) throw new Error(`session bundle agent ${index + 1} is unsupported`);
        const type = row.type as AgentType;
        const title = requiredString(row.title, `agent ${index + 1} title`);
        const resumeId = requiredString(row.resumeId, `agent ${index + 1} resume id`);
        const claim = `${type}\0${resumeId}`;
        if (claims.has(claim)) throw new Error(`duplicate agent session claim for ${type}`);
        claims.add(claim);
        return { type, title, resumeId };
    });

    return { session: { name, cwd, kind }, windows, agents };
}
