import { recordOf } from "../lib/guards";
import { applyPatch } from "../markdown/patch";
import type { MdChange } from "../markdown/types";
import type { QueuedMessage } from "./queuedMessages";
import { isAgentLetter } from "./agentLetters";
import { toolDiff, toolFailure } from "./diff";
import { toolPage } from "./pages";
import { toolOutput } from "./toolOutput";
import { toolRowArguments, toolRunning } from "./toolLabels";
import type {
    AccountMove,
    AcpAsyncTask,
    AcpAvailableCommand,
    AcpContentBlock,
    AcpContentChunk,
    AcpSubagent,
    AcpTaskNotice,
    AcpWorkflowAgent,
    AgentNotice,
    AcpToolCall,
    ChatAction,
    ChatFailure,
    ChatMessage,
    ChatPart,
    PartTree,
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
    earlierLost: false,
    turnOpening: null,
    retrying: null,
    queued: [],
};

/* The parent session and each subagent session own a transcript of the same
   shape, so streaming into one is the same work as streaming into the other. */
type Transcript = { messages: ChatMessage[]; nextId: number };

const textOf = (value: unknown): string | undefined => (typeof value === "string" ? value : undefined);

/* The agent echoes the notifications its harness writes to itself, and those
   are one block of tags with no prose around them. Markdown drops such a block
   whole, so echoing it would leave a bubble with nothing in it — the task it
   reports on already says its piece in the transcript. */
function isMarkupOnly(text: string): boolean {
    const trimmed = text.trim();
    if (!trimmed) return true;
    if (isAgentLetter(trimmed)) return false;
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

/* A part's tree follows the core's changes while each one fits the text it
   ends at; one that does not loses it until the core sends a whole tree. */
function nextTree(tree: PartTree | undefined, change: MdChange | undefined, text: string): PartTree | undefined {
    if (!change) return tree;
    const whole = change.o[0]?.p.length === 0 && change.o[0].at === 0 && "c" in change.o[0];
    if (!whole && !tree) return null;
    const blocks = applyPatch(whole ? [] : (tree?.blocks ?? []), change.o);
    return change.n === text.length ? { blocks, n: change.n } : null;
}

function appendChunk(
    transcript: Transcript,
    role: ChatMessage["role"],
    partKind: "text" | "thought",
    chunk: AcpContentChunk,
    timed: boolean,
    fresh: boolean,
    md?: MdChange,
): Transcript {
    const contentText = textOf(chunk.content.text);
    const lastMessage = transcript.messages.at(-1);
    const continues = lastMessage?.role === role && !lastMessage.id.startsWith("local-") && !fresh;
    const messageId = chunk.messageId ?? (continues ? lastMessage.id : `${role}-fallback-${transcript.nextId}`);
    const existingIndex = transcript.messages.findLastIndex((message) => message.id === messageId);
    const messages = [...transcript.messages];
    let nextId = transcript.nextId;

    const stream = timed && role === "assistant" && contentText !== undefined;

    if (existingIndex < 0) {
        const part: ChatPart =
            contentText !== undefined
                ? { id: `${messageId}-${partKind}-0`, kind: partKind, text: contentText, md: nextTree(undefined, md, contentText) }
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
        /* A task can end while the reply streams; its notice must not cut the reply's markdown in two. */
        let open = parts.length - 1;
        while (open > 0 && (parts[open].kind === "notice" || parts[open].kind === "agent_notice")) open -= 1;
        const last = parts[open];
        if (contentText !== undefined && last?.kind === partKind) {
            const text = last.text + contentText;
            parts[open] = { ...last, text, md: nextTree(last.md, md, text) };
        } else if (contentText !== undefined) {
            parts.push({
                id: `${messageId}-${partKind}-${parts.length}`,
                kind: partKind,
                text: contentText,
                md: nextTree(undefined, md, contentText),
            });
        } else {
            parts.push({ id: `${messageId}-content-${parts.length}`, kind: "content", content: boundContent(chunk.content) });
        }
        messages[existingIndex] = { ...message, parts, ...(stream ? timedStream(message, contentText.length) : {}) };
    }

    return { messages, nextId };
}

function appendPart(transcript: Transcript, part: ChatPart, fresh = false): Transcript {
    const messages = [...transcript.messages];
    const last = messages.at(-1);
    if (last?.role === "assistant" && !last.id.startsWith("local-") && !fresh) {
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

function upsertTool(transcript: Transcript, update: AcpToolCall, merge: boolean, fresh: boolean): Transcript | null {
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
        parts[partIndex] =
            current.endedAt !== undefined
                ? { ...current, tool: withoutPayload(tool) }
                : !toolRunning(tool)
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
    const ended = !toolRunning(update);
    /* Reloading a session replays its whole history at once, so a call that is
       already finished the first time this side sees it never ran here. Stamping
       both ends now would time the replay rather than the call. */
    const opened: ToolPart = { id: `tool-${update.toolCallId}`, kind: "tool", tool: update, ...(ended ? {} : { startedAt: Date.now() }) };
    return appendPart(transcript, ended ? settleTool({ ...opened, endedAt: Date.now() }) : opened, fresh);
}

/** Applies the updates a session streams regardless of whose session it is. */
function transcriptUpdate(transcript: Transcript, update: Record<string, unknown>, timed: boolean, fresh = false): Transcript | null {
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
            const md = recordOf(recordOf(update._meta)?.sikemux)?.md as MdChange | undefined;
            return appendChunk(transcript, role, partKind, chunk, timed, fresh && role === "assistant", md);
        }
        case "tool_call": {
            const toolCallId = textOf(update.toolCallId);
            const title = textOf(update.title);
            return toolCallId && title ? upsertTool(transcript, { ...update, toolCallId, title }, false, fresh) : null;
        }
        case "tool_call_update": {
            const toolCallId = textOf(update.toolCallId);
            return toolCallId ? upsertTool(transcript, { ...update, toolCallId, title: textOf(update.title) ?? "" }, true, fresh) : null;
        }
        default:
            return null;
    }
}

/* History replayed from the agent's own files marks the first update of a
   turn the agent began by itself, since no prompt shows where it starts. */
function replayedTurnStart(update: Record<string, unknown>): { at: number | null } | null {
    const kept = recordOf(recordOf(update._meta)?.sikemux);
    if (kept?.opensTurn !== true) return null;
    return { at: typeof kept.at === "number" ? kept.at : null };
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

/* The agent sends no last word for work it was cut off in, so it would spin
   forever. A subagent can outlive its turn, so only `endSubagents` stops it. */
function settleParts(parts: ChatPart[], at: number, endSubagents: boolean): ChatPart[] | null {
    let changed = false;
    const settled = parts.map((part) => {
        if (part.kind === "tool") {
            if (!toolRunning(part.tool)) return part;
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

function settleState(state: ChatState): ChatState {
    const messages = settleMessages(state.messages, Date.now(), false);
    return messages ? { ...state, messages, revision: state.revision + 1 } : state;
}

/* A subagent can spawn subagents of its own, so its card holds theirs. */
function patchSubagentIn(messages: ChatMessage[], sessionId: string, patch: (subagent: AcpSubagent) => AcpSubagent): ChatMessage[] | null {
    for (let messageIndex = messages.length - 1; messageIndex >= 0; messageIndex -= 1) {
        const message = messages[messageIndex];
        for (let partIndex = message.parts.length - 1; partIndex >= 0; partIndex -= 1) {
            const part = message.parts[partIndex];
            if (part.kind !== "subagent") continue;
            let subagent: AcpSubagent | null = null;
            if (part.subagent.sessionId === sessionId) subagent = patch(part.subagent);
            else {
                const nested = patchSubagentIn(part.subagent.messages, sessionId, patch);
                if (nested) subagent = { ...part.subagent, messages: nested };
            }
            if (!subagent) continue;
            const parts = [...message.parts];
            parts[partIndex] = { ...part, subagent };
            const patched = [...messages];
            patched[messageIndex] = { ...message, parts };
            return patched;
        }
    }
    return null;
}

function patchSubagent(state: ChatState, sessionId: string, patch: (subagent: AcpSubagent) => AcpSubagent): ChatState | null {
    const messages = patchSubagentIn(state.messages, sessionId, patch);
    return messages ? { ...state, messages, revision: state.revision + 1 } : null;
}

const hasSubagent = (messages: ChatMessage[], sessionId: string) => patchSubagentIn(messages, sessionId, (subagent) => subagent) !== null;

const SUBAGENT_STATES: AcpSubagent["state"][] = ["running", "completed", "failed", "cancelled", "disconnected"];
const TASK_STATES: AcpAsyncTask["state"][] = ["running", "paused", "completed", "failed", "stopped"];

function withSubagent<T extends Transcript>(transcript: T, update: Record<string, unknown>, everywhere: ChatMessage[]): T | null {
    const sessionId = textOf(update.subagentSessionId);
    if (!sessionId || hasSubagent(everywhere, sessionId)) return null;
    const subagent: AcpSubagent = {
        sessionId,
        name: textOf(update.name) ?? "Subagent",
        task: textOf(update.task) ?? "",
        state: "running",
        messages: [],
        nextId: 1,
    };
    return { ...transcript, ...appendPart(transcript, { id: `subagent-${sessionId}`, kind: "subagent", subagent }) };
}

function setSubagentState(state: ChatState, update: Record<string, unknown>): ChatState {
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

function setSubagentUsage(state: ChatState, update: Record<string, unknown>): ChatState {
    const subagentSessionId = textOf(update.subagentSessionId);
    const tokens = update.tokens;
    if (!subagentSessionId || typeof tokens !== "number") return state;
    return patchSubagent(state, subagentSessionId, (subagent) => ({ ...subagent, tokens })) ?? state;
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
        owner: textOf(update.owner),
    };
    return { ...state, tasks: [...state.tasks, task], revision: state.revision + 1 };
}

const stampedAt = (update: Record<string, unknown>): number | null => {
    const at = recordOf(recordOf(update._meta)?.sikemux)?.at;
    return typeof at === "number" ? at : null;
};

/* Work that ends while the agent is idle is what wakes it, so its notice stands
   between the turns, saying why the next one began, rather than at the end of
   the last one. */
function addNotice(state: ChatState, taskId: string, notice: AcpTaskNotice, at: number | null, betweenTurns = !state.running): ChatState {
    const id = `notice-${taskId}`;
    if (state.messages.some((message) => message.parts.some((part) => part.id === id))) return state;
    const part: ChatPart = { id, kind: "notice", notice };
    if (!betweenTurns) return { ...state, ...appendPart(state, part), revision: state.revision + 1 };
    const message: ChatMessage = { id: `agent-part-${state.nextId}`, role: "assistant", parts: [part], opensTurn: { at } };
    return { ...state, messages: [...state.messages, message], nextId: state.nextId + 1, revision: state.revision + 1 };
}

const NOTICE_STATES: Record<string, AcpTaskNotice["state"]> = {
    completed: "completed",
    failed: "failed",
    stopped: "stopped",
    killed: "stopped",
    cancelled: "stopped",
};

/* Claude Code tells itself a background task ended in a tag the person never
   sees. History keeps it, and it is the only record of why the next turn began. */
function harnessNotice(state: ChatState, update: Record<string, unknown>): ChatState | null {
    const text = textOf(recordOf(update.content)?.text);
    if (!text?.includes("<task-notification>")) return null;
    const tag = (name: string) => new RegExp(`<${name}>([\\s\\S]*?)</${name}>`).exec(text)?.[1]?.trim();
    const taskId = tag("task-id");
    const noticeState = NOTICE_STATES[tag("status") ?? ""];
    if (!taskId || !noticeState) return state;
    const summary = tag("summary");
    const name = /"([^"]+)"/.exec(summary ?? "")?.[1] ?? summary ?? "Background task";
    // History marks the notice that woke the agent; one that came mid-turn belongs to that turn.
    const woke = !state.running && replayedTurnStart(update) !== null;
    const subagent = textOf(update.subagentSessionId);
    return addNotice(state, taskId, { name, state: noticeState, ...(subagent ? { subagent } : {}) }, stampedAt(update), woke);
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
        const notice: AcpTaskNotice = {
            name: ended.name,
            state: taskState,
            ...(ended.taskType ? { taskType: ended.taskType } : {}),
            ...(summary ? { summary } : {}),
        };
        const noted = addNotice(state, ended.asyncTaskId, notice, stampedAt(update));
        return { ...noted, tasks: state.tasks.filter((_, position) => position !== index), revision: state.revision + 1 };
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
        agents: Array.isArray(update.agents) ? (update.agents as AcpWorkflowAgent[]) : current.agents,
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
        failure: resumeOf(state.failure, update),
        ...appendPart(state, { id: `agent-notice-${state.revision}`, kind: "agent_notice", notice }),
        revision: state.revision + 1,
    };
}

/* A chat a usage limit stopped says when it resumes, or that it no longer will. */
function resumeOf(failure: ChatFailure | null, update: Record<string, unknown>): ChatFailure | null {
    if (!("resumesAt" in update) || failure?.kind !== "limit") return failure;
    const { resumesAt: _, ...rest } = failure;
    return typeof update.resumesAt === "number" ? { ...rest, resumesAt: update.resumesAt } : rest;
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

/* An agent says it is retrying as a warning-level failure, in the same shape a
   failed turn uses. It is not a failure yet: the turn runs on. */
function retryOf(update: Record<string, unknown>): ChatState["retrying"] {
    const notice = recordOf(recordOf(recordOf(recordOf(update._meta)?.jetbrains)?.air)?.sessionFailure);
    if (notice?.severity !== "warning") return null;
    return {
        attempt: typeof notice.attempt === "number" && notice.attempt > 0 ? notice.attempt : null,
        connection: notice.category === "connection",
    };
}

/* A cancelled turn stops only the subagents it was waiting on, and the agent
   says which. Background ones run on and report their own end. */
function endTurn(state: ChatState, stopReason: string | null, at = Date.now()): ChatState {
    const settled = settleState(state);
    const last = settled.messages.at(-1);
    // Only a turn this side watched run has a finish worth stamping.
    const messages = state.running && last?.role === "assistant" ? [...settled.messages.slice(0, -1), { ...last, endedAt: at }] : settled.messages;
    return {
        ...settled,
        messages,
        running: false,
        turnOpening: null,
        retrying: null,
        suppressUserEcho: false,
        permissions: [],
        stopReason,
        revision: state.revision + 1,
    };
}

function sessionUpdate(state: ChatState, sessionId: string, update: Record<string, unknown>, main = false): ChatState {
    if (update.sessionUpdate === "subagent_state_update") return setSubagentState(state, update);
    if (update.sessionUpdate === "subagent_usage") return setSubagentUsage(state, update);
    // No subagent shares the chat's own session, so its updates need no search through the subagents.
    const inSubagent = main
        ? null
        : patchSubagent(state, sessionId, (subagent) => {
              const next =
                  update.sessionUpdate === "subagent_spawned"
                      ? withSubagent(subagent, update, state.messages)
                      : transcriptUpdate(subagent, update, state.running);
              return next ? { ...subagent, ...next } : subagent;
          });
    if (inSubagent) return inSubagent;

    if (update.sessionUpdate === "user_message_chunk") {
        const noticed = harnessNotice(state, update);
        if (noticed) return noticed;
    }
    if (update.sessionUpdate === "user_message_chunk" && state.suppressUserEcho) return state;
    const opening = state.turnOpening ?? replayedTurnStart(update);
    const streamed = transcriptUpdate(state, update, state.running, opening !== null);
    if (streamed) {
        const last = streamed.messages.at(-1);
        const opened = opening !== null && streamed.messages.length > state.messages.length && last?.role === "assistant";
        const marked = opened ? { ...streamed, messages: [...streamed.messages.slice(0, -1), { ...last, opensTurn: opening }] } : streamed;
        return {
            ...state,
            ...recordedTime(marked, update, state.running),
            turnOpening: opened ? null : opening,
            suppressUserEcho: false,
            retrying: update.sessionUpdate === "user_message_chunk" ? state.retrying : null,
            revision: state.revision + 1,
        };
    }

    switch (update.sessionUpdate) {
        case "subagent_spawned": {
            const spawned = withSubagent(state, update, state.messages);
            return spawned ? { ...spawned, suppressUserEcho: false, revision: state.revision + 1 } : state;
        }
        case "async_task_spawned":
            return spawnTask(state, update);
        case "async_task_progress":
        case "async_task_state_update":
            return patchTask(state, update);
        case "task_notice": {
            const taskId = textOf(update.taskId);
            const noticeState = NOTICE_STATES[textOf(update.state) ?? ""];
            if (!taskId || !noticeState) return state;
            const summary = textOf(update.summary);
            const subagent = textOf(update.subagentSessionId);
            const notice: AcpTaskNotice = {
                name: textOf(update.name) ?? "Background task",
                state: noticeState,
                ...(summary ? { summary } : {}),
                ...(subagent ? { subagent } : {}),
            };
            return addNotice(state, taskId, notice, stampedAt(update));
        }
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
        case "session_info_update": {
            const retrying = state.running ? retryOf(update) : null;
            return {
                ...state,
                title: typeof update.title === "string" ? update.title : state.title,
                ...(retrying && { retrying }),
                revision: state.revision + 1,
            };
        }
        case "queued_messages":
            return { ...state, queued: queuedOf(update.messages) };
        case "earlier_turns_lost":
            return { ...state, earlierLost: true, revision: state.revision + 1 };
        case "message_rewound": {
            const messageId = textOf(update.messageId);
            return messageId ? rewind(state, messageId) : state;
        }
        default:
            return state;
    }
}

function queuedOf(value: unknown): QueuedMessage[] {
    if (!Array.isArray(value)) return [];
    return value.flatMap((item) => {
        const message = recordOf(item);
        if (typeof message?.id !== "string" || typeof message.text !== "string") return [];
        const paths = Array.isArray(message.paths) ? message.paths.filter((path): path is string => typeof path === "string") : [];
        const context = Array.isArray(message.context) ? (message.context as QueuedMessage["context"]) : [];
        return [{ id: message.id, text: message.text, paths, context }];
    });
}

/* Turns from before what the transcript holds go above it. A subagent drawn
   where it started takes the place of any card for it shown further down. */
function withEarlier(state: ChatState, messages: ChatMessage[]): ChatState {
    if (!messages.length) return state;
    const placed = new Set<string>();
    collectSubagents(messages, placed);
    return { ...state, messages: [...messages, ...withoutSubagents(state.messages, placed)], revision: state.revision + 1 };
}

function collectSubagents(messages: ChatMessage[], into: Set<string>) {
    for (const message of messages)
        for (const part of message.parts) {
            if (part.kind !== "subagent") continue;
            into.add(part.subagent.sessionId);
            collectSubagents(part.subagent.messages, into);
        }
}

function withoutSubagents(messages: ChatMessage[], placed: ReadonlySet<string>): ChatMessage[] {
    if (!placed.size) return messages;
    let changed = false;
    const kept = messages.flatMap((message): ChatMessage[] => {
        const parts = withoutSubagentParts(message.parts, placed);
        if (parts === message.parts) return [message];
        changed = true;
        return parts.length ? [{ ...message, parts }] : [];
    });
    return changed ? kept : messages;
}

function withoutSubagentParts(parts: ChatPart[], placed: ReadonlySet<string>): ChatPart[] {
    let changed = false;
    const kept = parts.flatMap((part): ChatPart[] => {
        if (part.kind !== "subagent") return [part];
        if (placed.has(part.subagent.sessionId)) {
            changed = true;
            return [];
        }
        const messages = withoutSubagents(part.subagent.messages, placed);
        if (messages === part.subagent.messages) return [part];
        changed = true;
        return [{ ...part, subagent: { ...part.subagent, messages } }];
    });
    return changed ? kept : parts;
}

export function chatReducer(state: ChatState, action: ChatAction): ChatState {
    switch (action.type) {
        case "earlier":
            return withEarlier(state, action.messages);
        case "config":
            return { ...state, setup: { ...state.setup, configOptions: action.options }, revision: state.revision + 1 };
        case "reset":
            /* A reconnect keeps the transcript on screen so the pane does not
               blank out while the session loads: the resumed session replays
               its own history, and the first update it sends takes over. */
            return action.hold && state.messages.length > 0
                ? { ...initialChatState, messages: state.messages, nextId: state.nextId, awaitingReplay: true }
                : initialChatState;
        case "status":
            return {
                /* The host ends the subagents itself, with an update every client
                   keeps, so a status never does. */
                ...(action.state === "stopped" || action.state === "error" ? settleState(state) : state),
                connection: action.state,
                running: action.state === "stopped" || action.state === "error" ? false : state.running,
                permissions: action.state === "stopped" || action.state === "error" ? [] : state.permissions,
                tasks: action.state === "stopped" || action.state === "error" ? [] : state.tasks,
                error: action.state === "error" ? state.error : null,
                failure: action.state === "error" ? state.failure : null,
            };
        case "ready":
            return {
                ...state,
                connection: "ready",
                capabilities: action.capabilities,
                setup: action.setup,
                running: action.running ?? state.running,
                error: null,
                failure: null,
            };
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
                action.main,
            );
        case "turn_started":
            return {
                ...state,
                running: true,
                turnOpening: { at: action.at ?? null },
                retrying: null,
                stopReason: null,
                error: null,
                failure: null,
                revision: state.revision + 1,
            };
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
        case "dismiss_error":
            return { ...state, error: null, failure: null, revision: state.revision + 1 };
        case "error":
            return {
                ...state,
                connection: state.connection === "ready" ? "ready" : "error",
                running: false,
                retrying: null,
                permissions: [],
                error: action.message,
                failure: action.failure ?? null,
                revision: state.revision + 1,
            };
    }
}
