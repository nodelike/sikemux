import { toolDiff, toolFailure } from "./diff";
import { toolPage } from "./pages";
import { toolOutput } from "./toolOutput";
import { toolRowArguments } from "./toolLabels";
import type {
    AccountMove,
    AcpAsyncTask,
    AcpAvailableCommand,
    AcpContentBlock,
    AcpContentChunk,
    AcpSubagent,
    AcpTaskNotice,
    AgentNotice,
    AcpToolCall,
    ChatAction,
    ChatMessage,
    ChatPart,
    ChatState,
    ContextUsage,
} from "./types";

export const initialChatState: ChatState = {
    connection: "connecting",
    messages: [],
    commands: [],
    permissions: [],
    tasks: [],
    capabilities: {},
    setup: {},
    plan: null,
    usage: null,
    running: false,
    suppressUserEcho: false,
    error: null,
    failure: null,
    title: null,
    stopReason: null,
    nextId: 1,
    revision: 0,
    awaitingReplay: false,
};

/* The parent session and each subagent session own a transcript of the same
   shape, so streaming into one is the same work as streaming into the other. */
type Transcript = { messages: ChatMessage[]; nextId: number };

const textOf = (value: unknown): string | undefined => (typeof value === "string" ? value : undefined);
const recordOf = (value: unknown): Record<string, unknown> | undefined =>
    typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;

/* The agent echoes the notifications its harness writes to itself, and those
   are one block of tags with no prose around them. Markdown drops such a block
   whole, so echoing it would leave a bubble with nothing in it — the task it
   reports on already says its piece in the transcript. */
function isMarkupOnly(text: string): boolean {
    const trimmed = text.trim();
    if (!trimmed) return true;
    if (isHarnessMarkup(trimmed)) return true;
    if (!trimmed.startsWith("<") || /\n\s*\n/.test(trimmed)) return false;
    return /^<([a-z][\w-]*)\b[^>]*>[\s\S]*<\/\1>$/i.test(trimmed);
}

/* Tags Claude Code writes to itself. A finished background agent's notice
   carries its whole report, paragraphs and all, so these are recognised by
   name rather than by looking like a single short block. */
const HARNESS_TAGS = new Set([
    "task-notification",
    "system-reminder",
    "local-command-caveat",
    "local-command-stdout",
    "local-command-stderr",
    "command-name",
    "command-message",
    "command-args",
]);

function isHarnessMarkup(text: string): boolean {
    let rest = text;
    while (rest) {
        const name = /^<([a-z][\w-]*)>/.exec(rest)?.[1];
        if (!name || !HARNESS_TAGS.has(name)) return false;
        const end = rest.indexOf(`</${name}>`);
        if (end < 0) return false;
        rest = rest.slice(end + name.length + 3).trimStart();
    }
    return true;
}

/* Claude records a stop as a user message, so replaying a session would show
   the person saying something they never typed. */
const isInterruptMarker = (text: string): boolean => /^\[Request interrupted by user[^\]]*\]$/.test(text.trim());

function contentChunk(update: Record<string, unknown>): AcpContentChunk | null {
    const content = recordOf(update.content);
    if (!content || typeof content.type !== "string") return null;
    return {
        content: content as AcpContentBlock,
        ...(typeof update.messageId === "string" ? { messageId: update.messageId } : {}),
    };
}

/* A picture bigger than the transcript can reasonably hold onto is kept by name
   instead of by its bytes: the row still says what it was. */
const MAX_IMAGE_DATA_CHARS = 2 * 1024 * 1024;

function boundContent(content: AcpContentBlock): AcpContentBlock {
    const data = textOf(content.data);
    if (content.type !== "image" || data === undefined || data.length <= MAX_IMAGE_DATA_CHARS) return content;
    const { data: _oversized, ...rest } = content;
    return rest;
}

/* Reloading a session replays its whole history in one burst, which would time
   the replay rather than the writing. Only a turn this side watched run is
   timed, and the turn being live is what says so. */
function timedStream(message: ChatMessage, chars: number): Partial<ChatMessage> {
    const now = Date.now();
    return {
        streamStartedAt: message.streamStartedAt ?? now,
        streamEndedAt: now,
        streamChars: (message.streamChars ?? 0) + chars,
    };
}

function appendChunk(
    transcript: Transcript,
    role: ChatMessage["role"],
    partKind: "text" | "thought",
    chunk: AcpContentChunk,
    timed: boolean,
): Transcript {
    const contentText = textOf(chunk.content.text);
    const lastMessage = transcript.messages.at(-1);
    const messageId =
        chunk.messageId ??
        (lastMessage?.role === role && !lastMessage.id.startsWith("local-") ? lastMessage.id : `${role}-fallback-${transcript.nextId}`);
    const existingIndex = transcript.messages.findLastIndex((message) => message.id === messageId);
    const messages = [...transcript.messages];
    let nextId = transcript.nextId;

    const stream = timed && role === "assistant" && contentText !== undefined;

    if (existingIndex < 0) {
        const part: ChatPart =
            contentText !== undefined
                ? { id: `${messageId}-${partKind}-0`, kind: partKind, text: contentText }
                : { id: `${messageId}-content-0`, kind: "content", content: boundContent(chunk.content) };
        const opened: ChatMessage = {
            id: messageId,
            role,
            parts: [part],
            ...(role === "user" && chunk.messageId ? { promptId: chunk.messageId } : {}),
            ...(timed ? { sentAt: Date.now() } : {}),
        };
        messages.push(stream ? { ...opened, ...timedStream(opened, contentText.length) } : opened);
        nextId += 1;
    } else {
        const message = messages[existingIndex];
        const parts = [...message.parts];
        const last = parts.at(-1);
        if (contentText !== undefined && last?.kind === partKind) {
            parts[parts.length - 1] = { ...last, text: last.text + contentText };
        } else if (contentText !== undefined) {
            parts.push({ id: `${messageId}-${partKind}-${parts.length}`, kind: partKind, text: contentText });
        } else {
            parts.push({ id: `${messageId}-content-${parts.length}`, kind: "content", content: boundContent(chunk.content) });
        }
        messages[existingIndex] = { ...message, parts, ...(stream ? timedStream(message, contentText.length) : {}) };
    }

    return { messages, nextId };
}

function appendPart(transcript: Transcript, part: ChatPart): Transcript {
    const messages = [...transcript.messages];
    const last = messages.at(-1);
    if (last?.role === "assistant" && !last.id.startsWith("local-")) {
        messages[messages.length - 1] = { ...last, parts: [...last.parts, part] };
        return { messages, nextId: transcript.nextId };
    }
    messages.push({ id: `agent-part-${transcript.nextId}`, role: "assistant", parts: [part] });
    return { messages, nextId: transcript.nextId + 1 };
}

type ToolPart = Extract<ChatPart, { kind: "tool" }>;

/* Everything a running call was handed and handed back, which the transcript
   reads once and then has no further use for, apart from the few arguments
   its row names. */
const TOOL_PAYLOAD_KEYS = ["content", "rawInput", "rawOutput"] as const;

function withoutPayload(tool: AcpToolCall): AcpToolCall {
    const kept: AcpToolCall = { ...tool };
    for (const key of TOOL_PAYLOAD_KEYS) delete kept[key];
    const shown = toolRowArguments(tool);
    if (shown) kept.rawInput = shown;
    return kept;
}

/* A call that has ended has said everything it is going to say. The change it
   made, what it printed and the message it failed with are worked out here,
   once, and what they were worked out from is let go of rather than carried
   for the rest of the session and read again on every redraw. */
function settleTool(part: ToolPart): ToolPart {
    const diff = toolDiff(part.tool);
    const output = toolOutput(part.tool);
    const page = toolPage(part.tool);
    const failure = output ? null : toolFailure(part.tool);
    return {
        ...part,
        tool: withoutPayload(part.tool),
        ...(diff ? { diff } : {}),
        ...(page ? { page } : output ? { output } : {}),
        ...(failure ? { failure } : {}),
    };
}

function upsertTool(transcript: Transcript, update: AcpToolCall, merge: boolean): Transcript | null {
    const messages = [...transcript.messages];
    for (let messageIndex = messages.length - 1; messageIndex >= 0; messageIndex -= 1) {
        const message = messages[messageIndex];
        const partIndex = message.parts.findIndex((part) => part.kind === "tool" && part.tool.toolCallId === update.toolCallId);
        if (partIndex < 0) continue;
        const parts = [...message.parts];
        const current = parts[partIndex];
        if (current.kind !== "tool") continue;
        const patch = Object.fromEntries(Object.entries(update).filter(([key, value]) => value !== undefined && !(key === "title" && value === "")));
        const tool = merge ? { ...current.tool, ...patch } : update;
        const ended = tool.status === "completed" || tool.status === "failed";
        parts[partIndex] =
            current.endedAt !== undefined
                ? { ...current, tool: withoutPayload(tool) }
                : ended
                  ? settleTool({ ...current, tool, endedAt: Date.now() })
                  : { ...current, tool };
        messages[messageIndex] = { ...message, parts };
        return { messages, nextId: transcript.nextId };
    }

    /* An update can land for a call this side never saw open — the adapter
       raises a subagent as its own session and drops the call that spawned it.
       With nothing to merge into and no title to show, a new row would be a
       blank line that spins forever, so let it pass. */
    if (!update.title) return null;
    const ended = update.status === "completed" || update.status === "failed";
    /* Reloading a session replays its whole history at once, so a call that is
       already finished the first time this side sees it never ran here. Stamping
       both ends now would time the replay rather than the call. */
    const opened: ToolPart = { id: `tool-${update.toolCallId}`, kind: "tool", tool: update, ...(ended ? {} : { startedAt: Date.now() }) };
    return appendPart(transcript, ended ? settleTool({ ...opened, endedAt: Date.now() }) : opened);
}

/** Applies the updates a session streams regardless of whose session it is. */
function transcriptUpdate(transcript: Transcript, update: Record<string, unknown>, timed: boolean): Transcript | null {
    switch (update.sessionUpdate) {
        case "user_message_chunk":
        case "agent_message_chunk":
        case "agent_thought_chunk": {
            const chunk = contentChunk(update);
            if (!chunk) return null;
            const role = update.sessionUpdate === "user_message_chunk" ? "user" : "assistant";
            if (
                role === "user" &&
                typeof chunk.content.text === "string" &&
                (isMarkupOnly(chunk.content.text) || isInterruptMarker(chunk.content.text))
            )
                return null;
            const partKind = update.sessionUpdate === "agent_thought_chunk" ? "thought" : "text";
            return appendChunk(transcript, role, partKind, chunk, timed);
        }
        case "tool_call": {
            const toolCallId = textOf(update.toolCallId);
            const title = textOf(update.title);
            return toolCallId && title ? upsertTool(transcript, { ...update, toolCallId, title }, false) : null;
        }
        case "tool_call_update": {
            const toolCallId = textOf(update.toolCallId);
            return toolCallId ? upsertTool(transcript, { ...update, toolCallId, title: textOf(update.title) ?? "" }, true) : null;
        }
        default:
            return null;
    }
}

/* History the agent kept says when each part of it happened. A turn this
   side watched is timed as it runs; one rebuilt from history takes its prompt's
   time and its answer's last. */
function recordedTime(transcript: Transcript, update: Record<string, unknown>, timed: boolean): Transcript {
    const at = recordOf(recordOf(update._meta)?.sikemux)?.at;
    const last = transcript.messages.at(-1);
    if (timed || typeof at !== "number" || !last) return transcript;
    if (last.role === "user" && last.sentAt !== undefined) return transcript;
    const stamped = last.role === "user" ? { ...last, sentAt: at } : { ...last, endedAt: at };
    return { ...transcript, messages: [...transcript.messages.slice(0, -1), stamped] };
}

const TOOL_ENDED = ["completed", "failed", "cancelled"];

/* The agent sends no last word for work it was cut off in, so it would spin
   forever. A subagent can outlive its turn, so only `endSubagents` stops it. */
function settleParts(parts: ChatPart[], at: number, endSubagents: boolean): ChatPart[] | null {
    let changed = false;
    const settled = parts.map((part) => {
        if (part.kind === "tool") {
            if (TOOL_ENDED.includes(part.tool.status ?? "pending")) return part;
            changed = true;
            return settleTool({ ...part, tool: { ...part.tool, status: "cancelled" }, endedAt: part.endedAt ?? at });
        }
        if (part.kind !== "subagent") return part;
        const running = part.subagent.state === "running";
        if (running && !endSubagents) return part;
        const messages = settleMessages(part.subagent.messages, at, true);
        if (!messages && !running) return part;
        changed = true;
        return {
            ...part,
            subagent: { ...part.subagent, ...(running ? { state: "cancelled" as const } : {}), ...(messages ? { messages } : {}) },
        };
    });
    return changed ? settled : null;
}

function settleMessages(messages: ChatMessage[], at: number, endSubagents: boolean): ChatMessage[] | null {
    let changed = false;
    const settled = messages.map((message) => {
        const parts = settleParts(message.parts, at, endSubagents);
        if (!parts) return message;
        changed = true;
        return { ...message, parts };
    });
    return changed ? settled : null;
}

function settleState(state: ChatState, endSubagents: boolean): ChatState {
    const messages = settleMessages(state.messages, Date.now(), endSubagents);
    return messages ? { ...state, messages, revision: state.revision + 1 } : state;
}

function stopSubagents(state: ChatState): ChatState {
    let changed = false;
    const at = Date.now();
    const messages = state.messages.map((message) => {
        if (!message.parts.some((part) => part.kind === "subagent" && part.subagent.state === "running")) return message;
        changed = true;
        const parts = message.parts.map((part) => {
            if (part.kind !== "subagent" || part.subagent.state !== "running") return part;
            const settled = settleMessages(part.subagent.messages, at, true);
            return { ...part, subagent: { ...part.subagent, state: "cancelled" as const, ...(settled ? { messages: settled } : {}) } };
        });
        return { ...message, parts };
    });
    return changed ? { ...state, messages, revision: state.revision + 1 } : state;
}

function findSubagent(messages: ChatMessage[], sessionId: string): { messageIndex: number; partIndex: number } | null {
    for (let messageIndex = messages.length - 1; messageIndex >= 0; messageIndex -= 1) {
        const partIndex = messages[messageIndex].parts.findIndex((part) => part.kind === "subagent" && part.subagent.sessionId === sessionId);
        if (partIndex >= 0) return { messageIndex, partIndex };
    }
    return null;
}

function patchSubagent(state: ChatState, sessionId: string, patch: (subagent: AcpSubagent) => AcpSubagent): ChatState | null {
    const found = findSubagent(state.messages, sessionId);
    if (!found) return null;
    const message = state.messages[found.messageIndex];
    const current = message.parts[found.partIndex];
    if (current.kind !== "subagent") return null;
    const parts = [...message.parts];
    parts[found.partIndex] = { ...current, subagent: patch(current.subagent) };
    const messages = [...state.messages];
    messages[found.messageIndex] = { ...message, parts };
    return { ...state, messages, revision: state.revision + 1 };
}

const SUBAGENT_STATES: AcpSubagent["state"][] = ["running", "completed", "failed", "cancelled", "disconnected"];
const TASK_STATES: AcpAsyncTask["state"][] = ["running", "paused", "completed", "failed", "stopped"];

function spawnSubagent(state: ChatState, update: Record<string, unknown>): ChatState {
    const sessionId = textOf(update.subagentSessionId);
    if (!sessionId || findSubagent(state.messages, sessionId)) return state;
    const subagent: AcpSubagent = {
        sessionId,
        name: textOf(update.name) ?? "Subagent",
        task: textOf(update.task) ?? "",
        state: "running",
        messages: [],
        nextId: 1,
    };
    return {
        ...state,
        ...appendPart(state, { id: `subagent-${sessionId}`, kind: "subagent", subagent }),
        suppressUserEcho: false,
        revision: state.revision + 1,
    };
}

function spawnTask(state: ChatState, update: Record<string, unknown>): ChatState {
    const asyncTaskId = textOf(update.asyncTaskId);
    if (!asyncTaskId || state.tasks.some((task) => task.asyncTaskId === asyncTaskId)) return state;
    const task: AcpAsyncTask = {
        asyncTaskId,
        name: textOf(update.name) ?? "Background task",
        taskType: textOf(update.taskType) ?? "",
        description: textOf(update.description) ?? "",
        state: "running",
        canStop: update.canStop === true,
        outputFilePath: textOf(update.outputFilePath),
    };
    return { ...state, tasks: [...state.tasks, task], revision: state.revision + 1 };
}

function patchTask(state: ChatState, update: Record<string, unknown>): ChatState {
    const asyncTaskId = textOf(update.asyncTaskId);
    const index = state.tasks.findIndex((task) => task.asyncTaskId === asyncTaskId);
    if (index < 0) return state;
    const taskState = TASK_STATES.find((candidate) => candidate === update.state);

    /* A task that reached its end has nothing left to watch or stop, so it
       leaves the composer and says how it went in the transcript instead. */
    if (taskState === "completed" || taskState === "failed" || taskState === "stopped") {
        const ended = state.tasks[index];
        const summary = textOf(update.summary) ?? ended.summary;
        const notice: AcpTaskNotice = { name: ended.name, state: taskState, ...(summary ? { summary } : {}) };
        return {
            ...state,
            ...appendPart(state, { id: `notice-${ended.asyncTaskId}`, kind: "notice", notice }),
            tasks: state.tasks.filter((_, position) => position !== index),
            revision: state.revision + 1,
        };
    }

    const current = state.tasks[index];
    const tasks = [...state.tasks];
    tasks[index] = {
        ...current,
        state: taskState ?? current.state,
        description: textOf(update.description) ?? current.description,
        summary: textOf(update.summary) ?? current.summary,
        lastToolName: textOf(update.lastToolName) ?? current.lastToolName,
        outputFilePath: textOf(update.outputFilePath) ?? current.outputFilePath,
        usage: (recordOf(update.usage) as AcpAsyncTask["usage"]) ?? current.usage,
    };
    return { ...state, tasks, revision: state.revision + 1 };
}

function agentNotice(state: ChatState, update: Record<string, unknown>): ChatState {
    const title = textOf(update.title);
    /* The stopped task's own line already says so. */
    if (!title || title === "Task stopped by user") return state;
    const severity = update.severity === "warning" || update.severity === "error" ? update.severity : "info";
    const description = textOf(update.description);
    const notice: AgentNotice = { severity, title, ...(description ? { description } : {}) };
    return {
        ...state,
        ...appendPart(state, { id: `agent-notice-${state.revision}`, kind: "agent_notice", notice }),
        revision: state.revision + 1,
    };
}

/* The person took the chat back to before one of their messages: it goes, and
   everything after it with it. */
function rewind(state: ChatState, messageId: string): ChatState {
    const index = state.messages.findIndex((message) => message.role === "user" && (message.promptId ?? message.id) === messageId);
    if (index < 0) return state;
    return { ...state, messages: state.messages.slice(0, index), plan: null, stopReason: null, revision: state.revision + 1 };
}

function contextUsage(update: Record<string, unknown>): ContextUsage | null {
    const { used, size } = update;
    if (typeof used !== "number" || typeof size !== "number" || !Number.isFinite(used) || !Number.isFinite(size) || size <= 0) return null;
    const cost = recordOf(update.cost);
    return typeof cost?.amount === "number" && Number.isFinite(cost.amount) && typeof cost.currency === "string"
        ? { used, size, cost: { amount: cost.amount, currency: cost.currency } }
        : { used, size };
}

/* A cancelled turn takes its subagents with it, and the agent says so itself;
   settling them here as well covers one that does not. */
function endTurn(state: ChatState, stopReason: string | null, at = Date.now()): ChatState {
    const settled = settleState(state, stopReason === "cancelled");
    const last = settled.messages.at(-1);
    // Only a turn this side watched run has a finish worth stamping.
    const messages = state.running && last?.role === "assistant" ? [...settled.messages.slice(0, -1), { ...last, endedAt: at }] : settled.messages;
    return {
        ...settled,
        messages,
        running: false,
        suppressUserEcho: false,
        permissions: [],
        stopReason,
        revision: state.revision + 1,
    };
}

function sessionUpdate(state: ChatState, sessionId: string, update: Record<string, unknown>): ChatState {
    const inSubagent = patchSubagent(state, sessionId, (subagent) => {
        const next = transcriptUpdate(subagent, update, state.running);
        return next ? { ...subagent, ...next } : subagent;
    });
    if (inSubagent) return inSubagent;

    if (update.sessionUpdate === "user_message_chunk" && state.suppressUserEcho) return state;
    const streamed = transcriptUpdate(state, update, state.running);
    if (streamed) return { ...state, ...recordedTime(streamed, update, state.running), suppressUserEcho: false, revision: state.revision + 1 };

    switch (update.sessionUpdate) {
        case "subagent_spawned":
            return spawnSubagent(state, update);
        case "subagent_state_update": {
            const subagentSessionId = textOf(update.subagentSessionId);
            const subagentState = SUBAGENT_STATES.find((candidate) => candidate === update.state);
            if (!subagentSessionId || !subagentState) return state;
            return (
                patchSubagent(state, subagentSessionId, (subagent) => {
                    const messages = subagentState === "running" ? null : settleMessages(subagent.messages, Date.now(), true);
                    return { ...subagent, state: subagentState, ...(messages ? { messages } : {}) };
                }) ?? state
            );
        }
        case "async_task_spawned":
            return spawnTask(state, update);
        case "async_task_progress":
        case "async_task_state_update":
            return patchTask(state, update);
        case "notice":
            return agentNotice(state, update);
        case "plan":
            return { ...state, plan: update, suppressUserEcho: false, revision: state.revision + 1 };
        case "available_commands_update":
            return {
                ...state,
                commands: Array.isArray(update.availableCommands)
                    ? update.availableCommands
                          .filter(
                              (command): command is AcpAvailableCommand =>
                                  typeof command === "object" &&
                                  command !== null &&
                                  typeof (command as AcpAvailableCommand).name === "string" &&
                                  typeof (command as AcpAvailableCommand).description === "string",
                          )
                          .slice(0, 256)
                    : [],
                revision: state.revision + 1,
            };
        case "config_option_update":
            return { ...state, setup: { ...state.setup, configOptions: update.configOptions }, revision: state.revision + 1 };
        case "usage_update": {
            const usage = contextUsage(update);
            return usage ? { ...state, usage, revision: state.revision + 1 } : state;
        }
        case "account_switched": {
            const label = textOf(update.label);
            if (!label) return state;
            const from = textOf(update.from);
            const move: AccountMove = { label, ...(from ? { from } : {}), reason: update.reason === "limit" ? "limit" : "chosen" };
            return { ...state, ...appendPart(state, { id: `account-${state.nextId}`, kind: "account", move }), revision: state.revision + 1 };
        }
        case "session_info_update":
            return { ...state, title: typeof update.title === "string" ? update.title : state.title, revision: state.revision + 1 };
        case "message_rewound": {
            const messageId = textOf(update.messageId);
            return messageId ? rewind(state, messageId) : state;
        }
        default:
            return state;
    }
}

export function chatReducer(state: ChatState, action: ChatAction): ChatState {
    switch (action.type) {
        case "config":
            return { ...state, setup: { ...state.setup, configOptions: action.options }, revision: state.revision + 1 };
        case "saved_usage":
            return state.usage ? state : { ...state, usage: action.usage, revision: state.revision + 1 };
        case "reset":
            /* A reconnect keeps the transcript on screen so the pane does not
               blank out while the session loads: the resumed session replays
               its own history, and the first update it sends takes over. */
            return action.hold && state.messages.length > 0
                ? { ...initialChatState, messages: state.messages, nextId: state.nextId, awaitingReplay: true }
                : initialChatState;
        case "status":
            return {
                /* Starting again, as when the chat moves to another account, is a
                   new agent that knows nothing of the old one's subagents. */
                ...(action.state === "stopped" || action.state === "error"
                    ? settleState(state, true)
                    : action.state === "starting"
                      ? stopSubagents(state)
                      : state),
                connection: action.state,
                running: action.state === "stopped" || action.state === "error" ? false : state.running,
                permissions: action.state === "stopped" || action.state === "error" ? [] : state.permissions,
                tasks: action.state === "stopped" || action.state === "error" ? [] : state.tasks,
                error: action.state === "error" ? state.error : null,
                failure: action.state === "error" ? state.failure : null,
            };
        case "ready":
            return { ...state, connection: "ready", capabilities: action.capabilities, setup: action.setup, error: null, failure: null };
        case "local_prompt": {
            const id = `local-${state.nextId}`;
            return {
                ...state,
                messages: [
                    ...state.messages,
                    {
                        id,
                        role: "user",
                        ...(action.messageId ? { promptId: action.messageId } : {}),
                        sentAt: action.at ?? Date.now(),
                        parts: action.text.trim() ? [{ id: `${id}-text`, kind: "text", text: action.text }] : [],
                        ...(action.paths.length ? { attachments: action.paths } : {}),
                        ...(action.context?.length ? { context: action.context.map(({ uri, title }) => ({ uri, title })) } : {}),
                    },
                ],
                nextId: state.nextId + 1,
                suppressUserEcho: true,
                running: true,
                error: null,
                failure: null,
                revision: state.revision + 1,
            };
        }
        case "rewind":
            return rewind(state, action.messageId);
        case "session_update":
            return sessionUpdate(
                state.awaitingReplay ? { ...state, messages: [], nextId: 1, plan: null, awaitingReplay: false } : state,
                action.sessionId,
                action.update,
            );
        case "turn_started":
            return { ...state, running: true, stopReason: null, error: null, failure: null, revision: state.revision + 1 };
        case "turn_completed":
            return endTurn(state, action.stopReason ?? null, action.at);
        case "permission_requested":
            return {
                ...state,
                permissions: [...state.permissions.filter((request) => request.requestId !== action.request.requestId), action.request],
                revision: state.revision + 1,
            };
        case "permission_cleared":
            return {
                ...state,
                permissions: state.permissions.filter((request) => request.requestId !== action.requestId),
                revision: state.revision + 1,
            };
        case "error":
            return {
                ...state,
                connection: state.connection === "ready" ? "ready" : "error",
                running: false,
                permissions: [],
                error: action.message,
                failure: action.failure ?? null,
                revision: state.revision + 1,
            };
    }
}
