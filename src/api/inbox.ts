import type { AgentMessageLog } from "../agents/agentMessages";
import { invokeCommand as invoke } from "./invoke";

export type InboxTier = "needs" | "review" | "finished" | "progress";

export type InboxKind = "permission" | "question" | "input" | "held" | "assumption" | "pullRequest" | "changes" | "failed" | "finished" | "working";

export interface InboxRecording {
    path: string;
    seconds: number;
    at: number;
}

export interface InboxHandoff {
    answer: string | null;
    result: string | null;
    /** The first files the turn changed, as absolute paths. */
    files: string[];
    edited: number;
}

export interface PermissionChoices {
    allowOnce: string | null;
    allowAlways: string | null;
    reject: string | null;
}

/** One thing that waits on the person, as the core words and ranks it for every device. */
export interface InboxItem {
    id: string;
    tier: InboxTier;
    kind: InboxKind;
    agentId: string | null;
    name: string | null;
    cli: string | null;
    project: string | null;
    title: string | null;
    what: string;
    detail: string | null;
    at: number;
    card: { project: string; id: number; title: string } | null;
    requestId: string | null;
    choices: PermissionChoices | null;
    questionId: string | null;
    url: string | null;
    handoff: InboxHandoff | null;
    recordings: InboxRecording[];
    /** What this host calls the linked host the agent runs on, when it runs on one. */
    host?: string;
}

export type RemoteAgentStatus = "working" | "done" | "idle" | "blocked" | "stopped" | "starting" | "asleep";

/** A worker an agent here started on a linked host. */
export interface RemoteAgent {
    agentId: string;
    name: string;
    host: string;
    hostId: string;
    cli: string;
    status: RemoteAgentStatus;
    /** The agent here that started it. */
    lead: string | null;
    project: string;
    /** Its host answers now. */
    reachable: boolean;
}

export interface Inbox {
    items: InboxItem[];
    working: number;
    away: boolean;
    /** When each project, by its path, last logged a message between its agents. */
    agentMessages: Record<string, number>;
    /** Workers agents here started on linked hosts. */
    remoteAgents: RemoteAgent[];
}

/** Sent by the window whenever the core's inbox changes. */
export const INBOX_EVENT = "inbox_changed";

export const inboxApi = {
    read: () => invoke<Inbox>("inbox_call", { method: "inbox", params: null }),
    agentMessages: (project: string) => invoke<AgentMessageLog>("inbox_call", { method: "agentMessages", params: { project } }),
};
