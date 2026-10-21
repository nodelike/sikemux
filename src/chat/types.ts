import type { PageRef } from "../api/pages";
import type { ToolDiff } from "./diff";
import type { ToolOutput } from "./toolOutput";
import type { SentContext } from "./promptContext";
import type { QueuedMessage } from "./queuedMessages";
import type { MdElement } from "../markdown/types";

export interface AcpContentBlock {
    type: string;
    text?: string;
    data?: string;
    mimeType?: string;
    name?: string;
    title?: string;
    uri?: string;
    resource?: unknown;
    [key: string]: unknown;
}

export interface AcpContentChunk {
    content: AcpContentBlock;
    messageId?: string;
}

export interface AcpAvailableCommand {
    name: string;
    description: string;
    input?: { hint?: string };
}

export interface AcpPermissionOption {
    optionId: string;
    name: string;
    kind: "allow_once" | "allow_always" | "reject_once" | "reject_always" | string;
}

export interface AcpPermissionRequest {
    requestId: string;
    sessionId: string;
    toolCall: AcpToolCall;
    options: AcpPermissionOption[];
}

export interface AcpToolCall {
    toolCallId: string;
    title: string;
    kind?: string;
    status?: string;
    content?: unknown[];
    locations?: unknown[];
    rawInput?: unknown;
    rawOutput?: unknown;
    [key: string]: unknown;
}

export interface AcpAsyncTask {
    asyncTaskId: string;
    name: string;
    taskType: string;
    description: string;
    state: "running" | "paused" | "completed" | "failed" | "stopped";
    canStop: boolean;
    summary?: string;
    lastToolName?: string;
    outputFilePath?: string;
    usage?: { totalTokens: number; toolUses: number; durationMs: number };
    /** The subagent that started it, when the chat itself did not. */
    owner?: string;
    /** The agents a workflow has started so far. */
    agents?: AcpWorkflowAgent[];
}

export interface AcpWorkflowAgent {
    index: number;
    label: string;
    state: "queued" | "running" | "completed" | "failed";
    phase?: string;
    activity?: string;
    error?: string;
    tokens?: number;
    toolCalls?: number;
}

/* A subagent runs as its own ACP session, so its transcript is kept whole
   rather than spliced into the parent's. */
export interface AcpSubagent {
    sessionId: string;
    name: string;
    task: string;
    state: "running" | "completed" | "failed" | "cancelled" | "disconnected";
    messages: ChatMessage[];
    nextId: number;
    tokens?: number;
}

/* What a background task left behind once it ended, kept in the transcript
   because the live task above the composer goes away with it. */
export interface AcpTaskNotice {
    name: string;
    state: "completed" | "failed" | "stopped";
    summary?: string;
    taskType?: string;
    /* A subagent's notice shows that subagent's own card. */
    subagent?: string;
}

/** Something the agent tells the person outside its reply, like a model fallback. */
export interface AgentNotice {
    severity: "info" | "warning" | "error";
    title: string;
    description?: string;
}

/** The chat moved to another of the person's accounts, by itself at a usage limit or when asked. */
export interface AccountMove {
    label: string;
    from?: string;
    reason: "limit" | "chosen";
}

/** Why the last turn failed, when it was the account's or the network's doing. */
export interface ChatFailure {
    kind: "signIn" | "limit" | "network" | "other";
    /** What the agent itself said, behind the plain words. */
    details?: string;
    /** The profile id of the account it failed on. */
    account: string | null;
    /** When the core sends the stopped prompt again, once a usage limit resets, in Unix milliseconds. */
    resumesAt?: number;
}

/* The core's markdown tree of a part's text and the length it covers, or null
   once a change from the core did not fit the text here. */
export type PartTree = { blocks: MdElement[]; n: number } | null;

export type ChatPart =
    | { id: string; kind: "text"; text: string; md?: PartTree }
    | { id: string; kind: "thought"; text: string; md?: PartTree }
    | { id: string; kind: "content"; content: AcpContentBlock }
    /* A finished call is read once and kept as what the transcript shows: the
       change it made, what it printed, and what it left behind when it failed. */
    | {
          id: string;
          kind: "tool";
          tool: AcpToolCall;
          diff?: ToolDiff;
          output?: ToolOutput;
          page?: PageRef;
          failure?: string;
          startedAt?: number;
          endedAt?: number;
      }
    | { id: string; kind: "subagent"; subagent: AcpSubagent }
    | { id: string; kind: "notice"; notice: AcpTaskNotice }
    | { id: string; kind: "agent_notice"; notice: AgentNotice }
    | { id: string; kind: "account"; move: AccountMove };

export interface ChatMessage {
    id: string;
    role: "user" | "assistant";
    /* What the agent knows a message the person sent by, for editing it. */
    promptId?: string;
    parts: ChatPart[];
    attachments?: string[];
    context?: SentContext[];
    /* When the first and last characters of a streamed answer landed, and how
       many arrived between them — what the transcript's speed reading is
       worked out from. A replayed message has none of these. */
    streamStartedAt?: number;
    streamEndedAt?: number;
    streamChars?: number;
    /* When this side saw the message begin. A replayed message has none, since
       the history a session replays carries no times. */
    sentAt?: number;
    /* When the turn this message closed finished, as this side saw it. */
    endedAt?: number;
    /* The agent began a turn with this message, and when, if the core said. A
       turn it began by itself follows the last one without a prompt between. */
    opensTurn?: { at: number | null };
}

export interface ChatState {
    connection: "connecting" | "starting" | "initializing" | "ready" | "stopped" | "error";
    messages: ChatMessage[];
    commands: AcpAvailableCommand[];
    permissions: AcpPermissionRequest[];
    tasks: AcpAsyncTask[];
    capabilities: Record<string, unknown>;
    setup: Record<string, unknown>;
    plan: unknown;
    usage: ContextUsage | null;
    running: boolean;
    suppressUserEcho: boolean;
    error: string | null;
    failure: ChatFailure | null;
    title: string | null;
    stopReason: string | null;
    nextId: number;
    revision: number;
    awaitingReplay: boolean;
    /* The host no longer keeps the turns before the first one shown. */
    earlierLost: boolean;
    /* A turn has begun and its first message has not arrived yet. */
    turnOpening: { at: number | null } | null;
    /* The agent is retrying a request the network or its service failed. */
    retrying: { attempt: number | null; connection: boolean } | null;
    /* What the person sent mid-turn, waiting in the host's queue to go out once the turn ends. */
    queued: QueuedMessage[];
}

export type ChatAction =
    | { type: "reset"; hold?: boolean }
    | { type: "config"; options: unknown }
    | { type: "status"; state: ChatState["connection"] }
    | {
          type: "ready";
          capabilities: Record<string, unknown>;
          setup: Record<string, unknown>;
          /** Whether the core says a turn is running, for a chat taken up whose replay may start partway through it. */
          running?: boolean;
      }
    | { type: "local_prompt"; text: string; paths: string[]; context?: SentContext[]; messageId?: string; at?: number }
    | { type: "rewind"; messageId: string }
    | { type: "earlier"; messages: ChatMessage[] }
    | { type: "session_update"; sessionId: string; update: Record<string, unknown>; main?: boolean }
    | { type: "turn_started"; at?: number }
    | { type: "turn_completed"; stopReason?: string; at?: number }
    | { type: "permission_requested"; request: AcpPermissionRequest }
    | { type: "permission_cleared"; requestId: string }
    | { type: "error"; message: string; failure?: ChatFailure }
    | { type: "dismiss_error" };

/* One run of a code fence that reads as one colour, and the lines they make up.
   A run with nothing set is plain text, which keeps the colour the stylesheet
   already gives the fence. */
export interface CodeToken {
    readonly text: string;
    readonly color?: string;
    readonly italic?: boolean;
    readonly bold?: boolean;
    readonly underline?: boolean;
}

export type CodeLine = readonly CodeToken[];

/** How full the session's context window is, as the agent last reported it. */
export interface ContextUsage {
    used: number;
    size: number;
    cost?: { amount: number; currency: string };
}
