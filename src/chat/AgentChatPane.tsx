import {
    createContext,
    memo,
    useCallback,
    useContext,
    useEffect,
    useLayoutEffect,
    useMemo,
    useReducer,
    useRef,
    useState,
    type CSSProperties,
    type ReactNode,
    type RefObject,
} from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { useVirtualizer } from "@tanstack/react-virtual";
import { open } from "@tauri-apps/plugin-dialog";
import { acpApi, type AcpEvent } from "../api/acp";
import { fsapi } from "../api/fs";
import { invokeCommand as invoke } from "../api/invoke";
import { ComposerPickers, sessionConfigs, type SessionConfig } from "./ComposerPickers";
import { rateLabel, rowMeta } from "./messageMeta";
import { CopyButton } from "../components/CopyButton";
import { MarkdownTableHead } from "../lib/markdownTable";
import { basename } from "../lib/paths";
import { hasPrimaryModifier, PRIMARY_SHORTCUT } from "../lib/platform";
import { registerPathDrop } from "../state/dropRegistry";
import type { Agent, ProviderProfile } from "../state/types";
import * as cmd from "../state/commands";
import { swallow } from "../state/toast";
import { useStore } from "../state/store";
import {
    AgentIcon,
    IconAgent,
    IconArrowDown,
    IconArrowUp,
    IconCheck,
    IconChevron,
    IconClock,
    IconClose,
    IconCommand,
    IconFile,
    IconGlobe,
    IconPencil,
    IconPlug,
    IconPlus,
    IconSearch,
    IconShieldBolt,
    IconTimer,
    IconWarning,
} from "../components/Icons";
import { chatReducer, initialChatState } from "./reducer";
import { collapseDiff, fencedDiff, type DiffLine, type ToolDiff } from "./diff";
import { CodeRun, CodeTokens, fenceLanguage, splitAtMark, useCodeTokens, useDiffTokens } from "./codeHighlight";
import type { CodeLine } from "./types";
import { localImagePath, localPath, useImagePreview } from "./imagePreview";
import { ChatFileRef, PathRootsProvider, useFileRef } from "./FileRef";
import { YoloToggle } from "./YoloToggle";
import { ContextMeter } from "./ContextMeter";
import { guessClaudeWindow } from "./contextWindow";
import { agentApi } from "../api/agents";
import { safeWebUrl } from "../terminal/interactions";
import { chatUrlTransform, PATH_CLASS, PATH_CODE_CLASS, remarkFilePaths } from "./remarkFilePaths";
import { remarkHtmlAsText } from "./remarkHtmlAsText";
import { FoldMemoryContext, newFoldMemory, useLongTextFold } from "./longText";
import { imagesInClipboard, savePastedClipboard } from "./pasteImage";
import { showImage } from "../state/imageViewer";
import type {
    AcpAsyncTask,
    AcpAvailableCommand,
    AcpPermissionRequest,
    AcpSubagent,
    AcpTaskNotice,
    AcpToolCall,
    ChatMessage,
    ChatPart,
    ChatState,
    ContextUsage,
} from "./types";

const MAX_ATTACHMENTS = 32;
const MAX_DETAIL_CHARS = 120_000;
const HIDDEN_FLUSH_MS = 250;
// How far above the last line still counts as reading the latest message.
const BOTTOM_SLACK = 72;
/* A session that drops comes back on its own. The waits grow so an agent that
   cannot come back stops trying and hands the decision over. */
const RECONNECT_DELAYS = [700, 2_000, 5_000, 12_000];

function recordOf(value: unknown): Record<string, unknown> | null {
    return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
}

function eventMessage(event: AcpEvent): string {
    return typeof event.payload.message === "string" ? event.payload.message : "ACP session failed";
}

function formatDetail(value: unknown): string {
    let formatted: string;
    try {
        formatted = JSON.stringify(value, null, 2) ?? String(value);
    } catch {
        formatted = String(value);
    }
    return formatted.length > MAX_DETAIL_CHARS ? `${formatted.slice(0, MAX_DETAIL_CHARS)}\n… output truncated` : formatted;
}

function permissionRequest(payload: Record<string, unknown>): AcpPermissionRequest | null {
    const requestId = typeof payload.requestId === "string" ? payload.requestId : null;
    const sessionId = typeof payload.sessionId === "string" ? payload.sessionId : null;
    const toolCall = recordOf(payload.toolCall);
    const options = Array.isArray(payload.options) ? payload.options : null;
    if (!requestId || !sessionId || !toolCall || typeof toolCall.toolCallId !== "string" || !options) return null;
    return {
        requestId,
        sessionId,
        toolCall: { ...toolCall, toolCallId: toolCall.toolCallId, title: typeof toolCall.title === "string" ? toolCall.title : "Agent tool" },
        options: options.flatMap((option) => {
            const row = recordOf(option);
            return row && typeof row.optionId === "string" && typeof row.name === "string" && typeof row.kind === "string"
                ? [{ optionId: row.optionId, name: row.name, kind: row.kind }]
                : [];
        }),
    };
}

function statusFromEvent(event: AcpEvent): "connecting" | "installing" | "starting" | "initializing" | "ready" | "stopped" | "error" {
    const value = event.payload.state;
    return value === "installing" || value === "starting" || value === "initializing" || value === "ready" || value === "stopped" || value === "error"
        ? value
        : "connecting";
}

function mergePaths(current: string[], incoming: readonly string[]): string[] {
    const merged = [...current];
    for (const path of incoming) {
        if (!path || path.includes("\0") || merged.includes(path)) continue;
        if (merged.length === MAX_ATTACHMENTS) break;
        merged.push(path);
    }
    return merged;
}

/* The command a draft is naming is the one the caret sits in, so a slash works
   part-way through a sentence and not only as the first thing typed. */
function slashTokenAt(text: string, caret: number): { start: number; needle: string } | null {
    if (caret <= 0) return null;
    const start = text.lastIndexOf("/", caret - 1);
    if (start < 0) return null;
    if (start > 0 && !/\s/.test(text[start - 1])) return null;
    const needle = text.slice(start + 1, caret);
    return /\s/.test(needle) ? null : { start, needle };
}

// Splits `mcp__server__tool` so the server name can be de-emphasized.
function toolLabel(title: string): { scope?: string; name: string } {
    const segments = title.split("__");
    return segments[0] === "mcp" && segments.length > 2 ? { scope: segments[1], name: segments.slice(2).join("__") } : { name: title };
}

const ACTIVITY_BY_KIND: Record<string, string> = {
    read: "Reading…",
    edit: "Editing…",
    delete: "Deleting…",
    move: "Moving…",
    search: "Searching…",
    execute: "Running a command…",
    think: "Thinking…",
    fetch: "Fetching…",
    switch_mode: "Switching mode…",
};

/* A tool titles itself with what it was handed — often a whole shell command.
   The running row is one line, so say what the agent is doing rather than
   quote it back. */
function activityLabel(tool: AcpToolCall): string {
    const byKind = ACTIVITY_BY_KIND[tool.kind ?? ""];
    if (byKind) return byKind;
    const name = toolLabel(tool.title).name.split("\n")[0].trim();
    return name.length > 0 && name.length <= 40 ? name : "Working…";
}

const KIND_WORDS: Record<string, string> = {
    read: "read",
    edit: "edit",
    delete: "delete",
    move: "move",
    search: "search",
    execute: "run",
    think: "think",
    fetch: "fetch",
    switch_mode: "mode",
};

function toolKind(tool: AcpToolCall): string {
    const byKind = KIND_WORDS[tool.kind ?? ""];
    if (byKind) return byKind;
    const { scope, name } = toolLabel(tool.title);
    return scope ?? name.split(/[\s(]/)[0].slice(0, 12).toLowerCase();
}

function ToolKindIcon({ tool, kind }: { tool: AcpToolCall; kind?: string }) {
    if (tool.status === "failed") return <IconWarning size={11} />;
    switch (kind) {
        case "mcp":
            return <IconPlug size={11} />;
        case "read":
            return <IconFile size={11} />;
        case "search":
            return <IconSearch size={11} />;
        case "edit":
        case "move":
        case "delete":
            return <IconPencil size={11} />;
        case "execute":
            return <IconCommand size={11} />;
        case "fetch":
            return <IconGlobe size={11} />;
        default:
            return <IconAgent size={11} />;
    }
}

/* The row has one line for the target, so a path shows the name it ends in and
   keeps the rest in the tooltip. A command is not a path and stays as typed. */
function toolTarget(tool: AcpToolCall): string {
    const line = toolLabel(tool.title).name.split("\n")[0].trim();
    if (!line.includes("/") || /\s/.test(line)) return line;
    return basename(line) || line;
}

/* Which file a call was about: the one it reported touching, or the one its
   title names when it reported nothing. A shell command is not a file, and a
   title with a space in it is a command. */
function toolPath(tool: AcpToolCall): string | null {
    const first = Array.isArray(tool.locations) ? tool.locations[0] : null;
    if (first && typeof first === "object") {
        const { path, line } = first as { path?: unknown; line?: unknown };
        if (typeof path === "string" && path) return typeof line === "number" ? `${path}:${line}` : path;
    }
    const named = toolLabel(tool.title).name.split("\n")[0].trim();
    return named.includes("/") && !/\s/.test(named) ? named : null;
}

export function durationLabel(ms: number): string {
    // Tool calls are often quicker than a tenth of a second, and rounding those
    // to seconds reported every one of them as the same 0.0s.
    const elapsed = Math.max(0, ms);
    if (elapsed < 1000) return `${Math.round(elapsed)}ms`;
    const seconds = Math.round(elapsed / 1000);
    return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${String(seconds % 60).padStart(2, "0")}s`;
}

/* The text of one diff line: its runs, with the span that changed inside the
   one mark that shows it. A line with no colours yet is its own single run, so
   the marking is the same either way. */
function DiffText({ line, tokens }: { line: DiffLine; tokens?: CodeLine }) {
    const { pre, marked, post } = splitAtMark(tokens ?? [{ text: line.text }], line.mark);
    return (
        <span>
            <CodeRun tokens={pre} />
            {marked.length > 0 && (
                <mark>
                    <CodeRun tokens={marked} />
                </mark>
            )}
            <CodeRun tokens={post} />
        </span>
    );
}

function DiffBody({ diff }: { diff: ToolDiff }) {
    const [expanded, setExpanded] = useState(false);
    const view = useMemo(() => collapseDiff(diff.lines, expanded ? Number.MAX_SAFE_INTEGER : 3), [diff.lines, expanded]);
    const coloured = useDiffTokens(diff.lines, diff.path);
    return (
        <div className="chat-diff">
            <div className="chat-diff-head">
                <IconFile size={10} />
                <span className="chat-diff-path" title={diff.path}>
                    {diff.path}
                </span>
                <span className="chat-diff-adds">+{diff.adds}</span>
                <span className="chat-diff-dels">−{diff.dels}</span>
            </div>
            <div className="chat-diff-body">
                {view.rows.map((row, index) =>
                    "gap" in row ? (
                        <button type="button" className="chat-diff-gap" key={`gap-${index}`} onClick={() => setExpanded(true)}>
                            {row.gap} unchanged {row.gap === 1 ? "line" : "lines"}
                        </button>
                    ) : (
                        <div className={`chat-diff-line${row.sign === "+" ? " add" : row.sign === "-" ? " del" : ""}`} key={index}>
                            <span className="chat-diff-ln">{row.sign === "+" ? row.newLine : row.oldLine}</span>
                            <span className="chat-diff-sign">{row.sign}</span>
                            <DiffText line={row} tokens={coloured?.get(row)} />
                        </div>
                    ),
                )}
            </div>
        </div>
    );
}

function ToolRow({ part }: { part: Extract<ChatPart, { kind: "tool" }> }) {
    const [open, setOpen] = useState(false);
    const tool = part.tool;
    // An MCP call is named for the server it went to, whatever kind it claims.
    const rowKind = toolLabel(tool.title).scope !== undefined ? "mcp" : tool.kind;
    const diff = part.diff;
    const failure = part.failure;
    const detail = diff ?? failure;
    const status = tool.status ?? "pending";
    const file = useFileRef(toolPath(tool));
    /* A call the turn cut off has a duration, but printing it would read as a
       call that ran that long and then finished. It says why it stopped. */
    const measured = part.startedAt !== undefined && part.endedAt !== undefined ? part.endedAt - part.startedAt : null;
    const elapsed = status === "cancelled" ? "stopped" : measured !== null ? durationLabel(measured) : null;
    const toggle = () => setOpen((current) => !current);
    const lead = (
        <>
            <span className="chat-tool-tick" aria-hidden="true" />
            <span className="chat-tool-icon">
                <ToolKindIcon tool={tool} kind={rowKind} />
            </span>
            <span className="chat-tool-kind">{toolKind(tool)}</span>
            <span className="chat-tool-target">
                {file ? <ChatFileRef refers={file.ref} state={file.state} label={toolTarget(tool)} size={17} /> : toolTarget(tool)}
            </span>
        </>
    );
    const end = (
        <>
            {diff && (
                <span className="chat-tool-stat">
                    <span className="chat-diff-adds">+{diff.adds}</span>
                    <span className="chat-diff-dels">−{diff.dels}</span>
                </span>
            )}
            {elapsed ?? <span className="chat-tool-spinner" aria-hidden="true" />}
            {detail && <IconChevron size={10} className="chat-tool-chevron" />}
        </>
    );
    const rowProps = { className: `chat-tool status-${status}`, "data-kind": rowKind, title: tool.title };
    return (
        <div className="chat-tool-node">
            {/* A row whose target opens a file cannot itself be a button, so
                what is left of it opens the detail instead. */}
            {detail && !file ? (
                <button type="button" {...rowProps} aria-expanded={open} onClick={toggle}>
                    {lead}
                    <span className="chat-tool-end">{end}</span>
                </button>
            ) : (
                <div {...rowProps}>
                    {lead}
                    {detail ? (
                        <button
                            type="button"
                            className="chat-tool-end"
                            aria-expanded={open}
                            aria-label={open ? "Hide what the call did" : "Show what the call did"}
                            onClick={toggle}>
                            {end}
                        </button>
                    ) : (
                        <span className="chat-tool-end">{end}</span>
                    )}
                </div>
            )}
            {detail && open && (
                <div className="chat-tool-detail">{diff ? <DiffBody diff={diff} /> : <div className="chat-tool-out">{failure}</div>}</div>
            )}
        </div>
    );
}

const ChatAgentContext = createContext<{ id: string; type: Agent["type"] }>({ id: "", type: "claude" });

function openLink(href: string, agentId: string, external: boolean) {
    const path = localPath(href);
    const webUrl = safeWebUrl(href);
    if (path) void fsapi.revealInFinder(path).catch(swallow("reveal chat file"));
    else if (webUrl && agentId && !external) cmd.openUrlInBrowserPane(agentId, webUrl);
    else void invoke("open_url", { url: href, app: null, shortcut: null }).catch(swallow("open chat link"));
}

/* An attachment arrives as bytes and a type, with nothing naming it, so the
   type is the only thing that can say what it would be saved as. */
function attachmentName(mimeType: string): string {
    const kind =
        mimeType
            .split("/")
            .pop()
            ?.split("+")[0]
            ?.replace(/[^a-z0-9]/gi, "") || "png";
    return `attachment.${kind}`;
}

/* Every picture in a transcript is a thumbnail of itself: it opens at the size
   the window allows, where it can also be saved. */
function ChatImage({
    src,
    path,
    name = path ? basename(path) : "image.png",
    className = "chat-image",
}: {
    src: string;
    path?: string;
    name?: string;
    className?: string;
}) {
    return (
        <button type="button" className="chat-image-button" title={path ?? name} onClick={() => showImage({ src, name, path })}>
            <img className={className} alt={name} src={src} />
        </button>
    );
}

/* An agent writes an attached file back as a link to it. A picture beats its
   percent-encoded name, so show the picture whenever we can read it.

   A name the message only mentioned in passing arrives here too, marked as a
   guess. It is a file when the project has one by that name, and the words the
   agent wrote when it has not. */
function ChatLink({ href, className, children }: { href?: string; className?: string; children?: ReactNode }) {
    const guessed = className?.split(/\s+/) ?? [];
    const imagePath = localImagePath(href);
    const preview = useImagePreview(guessed.includes(PATH_CLASS) ? null : imagePath);
    const file = useFileRef(href);
    const agentId = useContext(ChatAgentContext).id;
    if (preview && imagePath) return <ChatImage src={preview} path={imagePath} />;
    if (file)
        return (
            <ChatFileRef
                refers={file.ref}
                state={file.state}
                label={children}
                className={guessed.includes(PATH_CODE_CLASS) ? "chat-file-ref code" : "chat-file-ref link"}
            />
        );
    if (guessed.includes(PATH_CODE_CLASS)) return <code>{children}</code>;
    if (guessed.includes(PATH_CLASS)) return <>{children}</>;
    return (
        <a
            href={href}
            onClick={(event) => {
                event.preventDefault();
                if (href) openLink(href, agentId, hasPrimaryModifier(event));
            }}>
            {children}
        </a>
    );
}

function codeText(children: ReactNode): string {
    if (typeof children === "string") return children;
    if (Array.isArray(children)) return children.map((child) => (typeof child === "string" ? child : "")).join("");
    return "";
}

function ChatCode({ className, children }: { className?: string; children?: ReactNode }) {
    const info = /language-(\S+)/.exec(className ?? "")?.[1];
    const text = codeText(children);
    const patch = useMemo(() => (text ? fencedDiff(text, info) : null), [text, info]);
    const tokens = useCodeTokens(text, patch ? null : fenceLanguage(info));
    // A patch in a fence is coloured the way the one in a tool call is, which
    // only happens at all when the fence says what file it is a patch to.
    const patchColours = useDiffTokens(patch, info);
    if (!info && !patch) return <code className={className}>{children}</code>;
    return (
        <>
            {info && <CodeTitle info={info} />}
            {patch ? (
                <code className={`${className ?? ""} chat-code-diff`}>
                    {patch.map((line, index) => (
                        <span className={`chat-diff-line${line.sign === "+" ? " add" : line.sign === "-" ? " del" : ""}`} key={index}>
                            <span className="chat-diff-sign">{line.sign}</span>
                            <DiffText line={line} tokens={patchColours?.get(line)} />
                        </span>
                    ))}
                </code>
            ) : tokens ? (
                <code className={className}>
                    <CodeTokens lines={tokens} />
                </code>
            ) : (
                <code className={className}>{children}</code>
            )}
        </>
    );
}

/* A fence says what file it quotes, when it says anything at all. The name is
   the file itself where the project has one; a bare language name is not. */
function CodeTitle({ info }: { info: string }) {
    const name = decodeURIComponent(info);
    const file = useFileRef(name);
    return (
        <span className="chat-code-title">
            {file ? (
                <ChatFileRef refers={file.ref} state={file.state} label={name} size={16} />
            ) : (
                <>
                    <IconFile size={10} />
                    {name}
                </>
            )}
        </span>
    );
}

function ChatTable({ children }: { children?: ReactNode }) {
    return (
        <div className="chat-table">
            <table>{children}</table>
        </div>
    );
}

const markdownComponents = { a: ChatLink, code: ChatCode, table: ChatTable, thead: MarkdownTableHead };
const remarkPlugins = [remarkGfm, remarkFilePaths];
const typedRemarkPlugins = [remarkGfm, remarkHtmlAsText, remarkFilePaths];

const MarkdownBody = memo(function MarkdownBody({ text, typed }: { text: string; typed: boolean }) {
    return (
        <Markdown remarkPlugins={typed ? typedRemarkPlugins : remarkPlugins} urlTransform={chatUrlTransform} skipHtml components={markdownComponents}>
            {text}
        </Markdown>
    );
});

/* A message still being written grows by a few characters a frame, and reading
   all of it again costs more the longer it gets. Ten times a second looks the
   same to a reader and leaves the frames between it free; the finished message
   is read once more in full. */
const LIVE_PARSE_MS = 100;

function LiveMarkdown({ text, live, typed = false }: { text: string; live: boolean; typed?: boolean }) {
    const [shown, setShown] = useState(text);
    const parsedAt = useRef(0);
    useEffect(() => {
        if (!live) {
            setShown(text);
            return;
        }
        const wait = LIVE_PARSE_MS - (Date.now() - parsedAt.current);
        if (wait <= 0) {
            parsedAt.current = Date.now();
            setShown(text);
            return;
        }
        const timer = window.setTimeout(() => {
            parsedAt.current = Date.now();
            setShown(text);
        }, wait);
        return () => window.clearTimeout(timer);
    }, [live, text]);
    return <MarkdownBody text={shown} typed={typed} />;
}

function ResourceLinkPart({ content }: { content: Extract<ChatPart, { kind: "content" }>["content"] }) {
    const uri = typeof content.uri === "string" ? content.uri : undefined;
    const imagePath = localImagePath(uri);
    const preview = useImagePreview(imagePath);
    const file = useFileRef(uri);
    if (preview && imagePath) return <ChatImage src={preview} path={imagePath} />;
    const label = content.title || content.name || uri || "Resource";
    return (
        <div className="chat-resource">
            {file ? (
                <ChatFileRef refers={file.ref} state={file.state} label={label} size={17} />
            ) : (
                <>
                    <IconFile size={13} />
                    <span>{label}</span>
                </>
            )}
        </div>
    );
}

function ContentPart({ part }: { part: Extract<ChatPart, { kind: "content" }> }) {
    const content = part.content;
    if (content.type === "resource_link") return <ResourceLinkPart content={content} />;
    /* A picture too big to keep was kept by name, so the row says what it was. */
    if (content.type === "image") {
        return typeof content.data === "string" && typeof content.mimeType === "string" ? (
            <ChatImage src={`data:${content.mimeType};base64,${content.data}`} name={attachmentName(content.mimeType)} />
        ) : (
            <ResourceLinkPart content={content} />
        );
    }
    return <pre className="chat-unknown-part">{formatDetail(content)}</pre>;
}

function FoldedMarkdown({ id, text, live, typed = false }: { id: string; text: string; live: boolean; typed?: boolean }) {
    const { cut, expand } = useLongTextFold(id, text, live);
    if (!cut) return <LiveMarkdown text={text} live={live} typed={typed} />;
    return (
        <>
            <LiveMarkdown text={cut.head} live={false} typed={typed} />
            <button type="button" className="chat-show-rest" onClick={expand}>
                Show the rest — {Math.round(cut.hidden / 1000)}k more characters
            </button>
        </>
    );
}

const MessagePart = memo(function MessagePart({ part, live, typed }: { part: ChatPart; live: boolean; typed: boolean }) {
    if (part.kind === "text") {
        return (
            <div className="chat-markdown">
                <FoldedMarkdown id={part.id} text={part.text} live={live} typed={typed} />
            </div>
        );
    }
    if (part.kind === "thought") {
        return (
            <div className="chat-thought">
                <div className="chat-markdown">
                    <FoldedMarkdown id={part.id} text={part.text} live={live} />
                </div>
            </div>
        );
    }
    if (part.kind === "tool") return <ToolRow part={part} />;
    if (part.kind === "subagent") return <SubagentPart subagent={part.subagent} />;
    if (part.kind === "notice") return <NoticePart notice={part.notice} />;
    return <ContentPart part={part} />;
});

function SentAttachment({ path }: { path: string }) {
    const preview = useImagePreview(path);
    const file = useFileRef(path);
    if (preview) return <ChatImage src={preview} path={path} className="chat-attachment-thumb" />;
    if (file) return <ChatFileRef refers={file.ref} state={file.state} label={basename(path)} />;
    return (
        <span title={path}>
            <IconFile size={12} />
            {basename(path)}
        </span>
    );
}

function ComposerAttachment({ path, onRemove }: { path: string; onRemove: () => void }) {
    const preview = useImagePreview(path);
    const remove = (
        <button type="button" aria-label={`Remove ${basename(path)}`} onClick={onRemove}>
            <IconClose size={11} />
        </button>
    );
    if (preview)
        return (
            <span className="image" title={path}>
                <img alt={basename(path)} src={preview} />
                {remove}
            </span>
        );
    return (
        <span title={path}>
            <IconFile size={14} />
            <span>{basename(path)}</span>
            {remove}
        </span>
    );
}

type PartGroup = { id: string; tools: Extract<ChatPart, { kind: "tool" }>[] } | { id: string; part: ChatPart };

function groupParts(parts: ChatPart[]): PartGroup[] {
    const groups: PartGroup[] = [];
    for (const part of parts) {
        const last = groups.at(-1);
        if (part.kind !== "tool") groups.push({ id: part.id, part });
        else if (last && "tools" in last) last.tools.push(part);
        else groups.push({ id: part.id, tools: [part] });
    }
    return groups;
}

function toolRunning(tool: AcpToolCall): boolean {
    const status = tool.status ?? "pending";
    return status !== "completed" && status !== "failed" && status !== "cancelled";
}

/* A run of tool calls is worth watching while the agent is still adding to it
   and worth folding away once it has moved on, so it stays open until
   something else follows it or the turn ends, unless the reader says
   otherwise. Watching each call instead would shut the run in the gaps
   between calls, and open it again on the next one. */
function ToolGroup({ tools, live }: { tools: Extract<ChatPart, { kind: "tool" }>[]; live: boolean }) {
    const [reader, setReader] = useState<boolean | null>(null);
    const running = tools.some((part) => toolRunning(part.tool));
    const open = reader ?? (live || running);
    const spent = tools.reduce(
        (total, part) => total + (part.startedAt !== undefined && part.endedAt !== undefined ? part.endedAt - part.startedAt : 0),
        0,
    );
    /* One column for every call in the run, as wide as the longest name in it:
       a run of reads stays tight, one that called an MCP server gets the room. */
    const kindWidth = Math.min(16, Math.max(4, ...tools.map((part) => toolKind(part.tool).length)));
    return (
        <div className="chat-tools">
            <button type="button" className="chat-tools-sum" aria-expanded={open} onClick={() => setReader(!open)}>
                <span className="chat-tools-count">
                    {tools.length} tool {tools.length === 1 ? "call" : "calls"}
                </span>
                {spent > 0 && <span className="chat-tools-time">{durationLabel(spent)}</span>}
                <IconChevron size={10} className="chat-tools-chevron" />
            </button>
            {open && (
                <div className="chat-tools-body" style={{ "--chat-kind": `${kindWidth}ch` } as CSSProperties}>
                    {tools.map((part) => (
                        <ToolRow key={part.id} part={part} />
                    ))}
                </div>
            )}
        </div>
    );
}

function PartGroups({ parts, live, typed = false }: { parts: ChatPart[]; live: boolean; typed?: boolean }) {
    const groups = groupParts(parts);
    return groups.map((group, index) =>
        "tools" in group ? (
            <ToolGroup key={group.id} tools={group.tools} live={live && index === groups.length - 1} />
        ) : (
            <MessagePart key={group.id} part={group.part} live={live && index === groups.length - 1} typed={typed} />
        ),
    );
}

function NoticePart({ notice }: { notice: AcpTaskNotice }) {
    return (
        <div className={`chat-notice state-${notice.state}`} role="status">
            <IconTimer size={12} />
            <span className="chat-notice-name">{notice.name}</span>
            <span className="chat-notice-state">{notice.state}</span>
            {notice.summary && <span className="chat-notice-summary">{notice.summary}</span>}
        </div>
    );
}

const SUBAGENT_WORDS: Record<AcpSubagent["state"], string> = {
    running: "working",
    completed: "done",
    failed: "failed",
    cancelled: "stopped",
    disconnected: "lost",
};

function SubagentStateMark({ state }: { state: AcpSubagent["state"] }) {
    const word = SUBAGENT_WORDS[state];
    return (
        <span className="chat-subagent-state" role="img" aria-label={word} title={word}>
            {state === "running" ? (
                <span className="chat-subagent-spinner" aria-hidden="true" />
            ) : state === "completed" ? (
                <IconCheck size={13} />
            ) : state === "failed" ? (
                <IconWarning size={12} />
            ) : state === "disconnected" ? (
                <IconPlug size={12} />
            ) : (
                <IconClose size={11} />
            )}
        </span>
    );
}

/* A subagent is handed a whole prompt as its task, and a prompt is paragraphs.
   The row is one line, so it opens with the first line and the tooltip keeps
   the rest. */
function subagentTask(task: string): string {
    return task.split("\n")[0].trim();
}

/* What a subagent is up to, taken from the last thing it sent. A tool it is
   part-way through says more than the prose it wrote before starting. */
function subagentActivity(subagent: AcpSubagent): string {
    for (let index = subagent.messages.length - 1; index >= 0; index -= 1) {
        const parts = subagent.messages[index].parts;
        for (let position = parts.length - 1; position >= 0; position -= 1) {
            const part = parts[position];
            if (part.kind === "tool") return `${toolKind(part.tool)} ${toolTarget(part.tool)}`.trim();
        }
    }
    return subagentTask(subagent.task);
}

/* A folded subagent keeps streaming into a transcript nobody is reading, so its
   body is only built once the reader opens it. */
function SubagentPart({ subagent }: { subagent: AcpSubagent }) {
    const agentType = useContext(ChatAgentContext).type;
    const [open, setOpen] = useState(false);
    const parts = subagent.messages.flatMap((message) => message.parts);
    const calls = parts.filter((part) => part.kind === "tool").length;
    return (
        <details className={`chat-subagent state-${subagent.state}`} open={open} onToggle={(event) => setOpen(event.currentTarget.open)}>
            <summary>
                <IconChevron size={9} className="chat-subagent-chevron" />
                <span className="chat-subagent-mark">
                    <AgentIcon type={agentType} size={18} className={`agent-glyph ${agentType}`} />
                </span>
                <span className="chat-subagent-name">{subagent.name}</span>
                <span className="chat-subagent-task" title={subagent.task || undefined}>
                    {subagentTask(subagent.task)}
                </span>
                <span className="chat-subagent-end">
                    {calls > 0 && (
                        <span className="chat-subagent-calls">
                            {calls} {calls === 1 ? "call" : "calls"}
                        </span>
                    )}
                    <SubagentStateMark state={subagent.state} />
                </span>
            </summary>
            {open && (
                <div className="chat-subagent-body">
                    {parts.length > 0 ? (
                        <PartGroups parts={parts} live={subagent.state === "running"} />
                    ) : (
                        <span className="chat-subagent-empty">No output yet.</span>
                    )}
                </div>
            )}
        </details>
    );
}

/* A task whose description repeats its name would print the same words twice,
   once in each voice, so the detail takes the first thing that says more. */
function taskDetail(task: AcpAsyncTask): string | undefined {
    return [task.summary, task.description, task.lastToolName, task.taskType].find((text) => text && text !== task.name);
}

function BackgroundTasks({ tasks, stopping, onStop }: { tasks: AcpAsyncTask[]; stopping: string[]; onStop: (taskId: string) => void }) {
    if (tasks.length === 0) return null;
    return (
        <>
            {groupTasks(tasks).map(([kind, group]) => (
                <Group label={kind} count={group.length} key={kind}>
                    {group.map((task) => (
                        <div className={`chat-task state-${task.state}`} key={task.asyncTaskId}>
                            {kind === "shell" ? <IconCommand size={12} /> : <IconTimer size={12} />}
                            <span className="chat-task-name">{task.name}</span>
                            <span className="chat-task-detail">{taskDetail(task)}</span>
                            {task.canStop && (
                                <button
                                    type="button"
                                    aria-label={`Stop ${task.name}`}
                                    disabled={stopping.includes(task.asyncTaskId)}
                                    onClick={() => onStop(task.asyncTaskId)}>
                                    <IconClose size={10} />
                                </button>
                            )}
                        </div>
                    ))}
                </Group>
            ))}
        </>
    );
}

function runningSubagents(messages: ChatMessage[]): AcpSubagent[] {
    const running: AcpSubagent[] = [];
    for (const message of messages)
        for (const part of message.parts) if (part.kind === "subagent" && part.subagent.state === "running") running.push(part.subagent);
    return running;
}

/* A subagent at work belongs where the reader already watches for live things
   — the strip over the composer that the background tasks use. Its card in the
   transcript is where its output went, which is not where you look to find out
   whether it is still going. */
function RunningSubagents({ subagents }: { subagents: AcpSubagent[] }) {
    const agentType = useContext(ChatAgentContext).type;
    if (subagents.length === 0) return null;
    return (
        <Group label="subagent" count={subagents.length}>
            {subagents.map((subagent) => (
                <div className="chat-task chat-task-agent" key={subagent.sessionId}>
                    <AgentIcon type={agentType} size={16} className={`agent-glyph ${agentType}`} />
                    <span className="chat-task-name">{subagent.name}</span>
                    <span className="chat-task-detail">{subagentActivity(subagent)}</span>
                    <span className="chat-task-spinner" aria-hidden="true" />
                </div>
            ))}
        </Group>
    );
}

/* One kind of running work, under a label that counts it. The label is what
   makes a stack of eight rows readable, so it stays even for a group of one.
   `plural` is for the kinds that are not a noun with an s on the end. */
function Group({ label, plural, count, children }: { label: string; plural?: string; count: number; children: ReactNode }) {
    const word = count === 1 ? label : (plural ?? `${label}s`);
    return (
        <div className="chat-group" aria-label={`${count} ${word}`}>
            <div className="chat-group-label">
                <span>{word}</span>
                <span className="chat-group-count">{count}</span>
            </div>
            {children}
        </div>
    );
}

/* An agent names its own task types — "shell", "monitor" — and they are the
   only thing that separates one background task from another, so they are what
   the groups are cut on. */
function groupTasks(tasks: AcpAsyncTask[]): [string, AcpAsyncTask[]][] {
    const groups = new Map<string, AcpAsyncTask[]>();
    for (const task of tasks) {
        const kind = task.taskType || "task";
        const existing = groups.get(kind);
        if (existing) existing.push(task);
        else groups.set(kind, [task]);
    }
    return [...groups];
}

type QueuedMessage = { id: string; text: string; paths: string[] };

const queuedLabel = (message: QueuedMessage): string => message.text || message.paths.map(basename).join(", ");

function QueuedMessages({
    messages,
    steerable,
    onSteer,
    onDrop,
}: {
    messages: QueuedMessage[];
    steerable: boolean;
    onSteer: (message: QueuedMessage) => void;
    onDrop: (id: string) => void;
}) {
    if (messages.length === 0) return null;
    return (
        <Group label="queued" plural="queued" count={messages.length}>
            {messages.map((message) => {
                const label = queuedLabel(message);
                return (
                    <div className="chat-queued-message" key={message.id}>
                        <IconClock size={12} />
                        <span className="chat-queued-text">{label}</span>
                        {steerable && (
                            <button
                                type="button"
                                className="chat-queued-steer"
                                aria-label={`Steer the running turn with ${label}`}
                                onClick={() => onSteer(message)}>
                                Steer
                                {messages.length === 1 && <kbd className="chat-queued-steer-key">{PRIMARY_SHORTCUT}↵</kbd>}
                            </button>
                        )}
                        <button type="button" aria-label={`Drop ${label} from the queue`} onClick={() => onDrop(message.id)}>
                            <IconClose size={10} />
                        </button>
                    </div>
                );
            })}
        </Group>
    );
}

function connectingLabel(connection: ChatState["connection"]): string | null {
    if (connection === "installing") return "Installing structured-session adapter…";
    if (connection === "starting") return "Starting agent adapter…";
    if (connection === "connecting" || connection === "initializing") return "Connecting to agent session…";
    return null;
}

function elapsedLabel(seconds: number): string {
    return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${String(seconds % 60).padStart(2, "0")}s`;
}

/* Keeps its own clock so a ticking second redraws this row alone, not the
   whole transcript. */
function ChatActivity({ label, agentType }: { label: string; agentType: Agent["type"] }) {
    const [seconds, setSeconds] = useState(0);
    useEffect(() => {
        const started = Date.now();
        const timer = window.setInterval(() => setSeconds(Math.round((Date.now() - started) / 1000)), 1000);
        return () => window.clearInterval(timer);
    }, []);
    return (
        <div className="chat-activity" role="status">
            <span className={`chat-activity-mark agent-glyph ${agentType}`} aria-hidden="true">
                <AgentIcon type={agentType} size={21} />
            </span>
            <span className="chat-activity-label">{label}</span>
            {seconds > 0 && (
                <span className="chat-activity-elapsed" aria-hidden="true">
                    {elapsedLabel(seconds)}
                </span>
            )}
        </div>
    );
}

const ChatMessageRow = memo(function ChatMessageRow({
    message,
    live,
    copyable,
    rate,
}: {
    message: ChatMessage;
    live: boolean;
    copyable: string;
    rate: number | null;
}) {
    return (
        <article className={`chat-message ${message.role}`}>
            <div className="chat-message-content">
                {message.attachments && message.attachments.length > 0 && (
                    <div className="chat-message-attachments">
                        {message.attachments.map((path) => (
                            <SentAttachment key={path} path={path} />
                        ))}
                    </div>
                )}
                <PartGroups parts={message.parts} live={live} typed={message.role === "user"} />
                {copyable && (
                    <div className="chat-message-meta">
                        <CopyButton value={copyable} label={message.role === "user" ? "message" : "reply"} size={15} />
                        {rate !== null && (
                            <span className="chat-message-rate" title="Writing speed, estimated from the text that arrived">
                                {rateLabel(rate)}
                            </span>
                        )}
                    </div>
                )}
            </div>
        </article>
    );
});

function SlashCommands({
    commands,
    selected,
    onSelect,
}: {
    commands: AcpAvailableCommand[];
    selected: number;
    onSelect: (command: AcpAvailableCommand) => void;
}) {
    return (
        <div className="chat-slash-menu" role="listbox" aria-label="Session commands">
            <div className="chat-slash-heading">
                <span>Session commands</span>
                <span>ACP</span>
            </div>
            {commands.map((command, index) => (
                <button
                    key={command.name}
                    type="button"
                    role="option"
                    aria-selected={index === selected}
                    className={index === selected ? "selected" : ""}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => onSelect(command)}>
                    <code>/{command.name}</code>
                    <span>{command.description}</span>
                    {command.input?.hint && <em>{command.input.hint}</em>}
                </button>
            ))}
        </div>
    );
}

function PermissionRequest({ request, busy, onReply }: { request: AcpPermissionRequest; busy: boolean; onReply: (optionId?: string) => void }) {
    return (
        <section className="chat-permission" aria-label={`Permission required for ${request.toolCall.title}`}>
            <div className="chat-permission-copy">
                <IconShieldBolt size={16} />
                <div>
                    <strong>{request.toolCall.title}</strong>
                    <span>Agent needs permission before this tool can continue.</span>
                </div>
            </div>
            <div className="chat-permission-actions">
                {request.options.map((option) => (
                    <button
                        key={option.optionId}
                        type="button"
                        disabled={busy}
                        className={option.kind.startsWith("reject") ? "reject" : "allow"}
                        onClick={() => onReply(option.optionId)}>
                        {option.name}
                    </button>
                ))}
                {!request.options.some((option) => option.kind.startsWith("reject")) && (
                    <button type="button" disabled={busy} className="reject" onClick={() => onReply()}>
                        Cancel
                    </button>
                )}
            </div>
        </section>
    );
}

/* The composer keeps the draft to itself: a keystroke redraws these few rows
   rather than the transcript above them. */
function ChatComposer({
    agent,
    profile,
    paneRef,
    visible,
    connection,
    running,
    steerable,
    commands,
    setup,
    awaitingPermission,
    agentLocked,
    changingConfig,
    changingPermissions,
    permissionApplied,
    placeholder,
    error,
    onError,
    onSend,
    onSteerQueued,
    onStop,
    queuedCount,
    usage,
    onConfig,
}: {
    agent: Agent;
    profile?: ProviderProfile;
    paneRef: RefObject<HTMLDivElement | null>;
    visible: boolean;
    connection: ChatState["connection"];
    running: boolean;
    steerable: boolean;
    commands: AcpAvailableCommand[];
    setup: Record<string, unknown>;
    awaitingPermission: boolean;
    agentLocked: boolean;
    changingConfig: boolean;
    changingPermissions: boolean;
    permissionApplied: boolean;
    placeholder: string;
    error: string | null;
    onError: (message: string | null) => void;
    onSend: (text: string, paths: string[], steerNow: boolean) => boolean;
    onSteerQueued: () => void;
    onStop: () => void;
    queuedCount: number;
    usage: ContextUsage | null;
    onConfig: (config: SessionConfig, value: string) => void;
}) {
    const [draft, setDraft] = useState("");
    const [caret, setCaret] = useState(0);
    const [attachments, setAttachments] = useState<string[]>([]);
    const [slashSelection, setSlashSelection] = useState(0);
    const [slashDismissed, setSlashDismissed] = useState(false);
    const editorRef = useRef<HTMLTextAreaElement>(null);

    /* The field grows with what is typed until it reaches its CSS max-height,
       and scrolls from there. */
    useLayoutEffect(() => {
        const editor = editorRef.current;
        if (!editor) return;
        editor.style.height = "auto";
        editor.style.height = `${editor.scrollHeight}px`;
    }, [draft]);

    useEffect(() => {
        const element = paneRef.current;
        if (!element) return;
        return registerPathDrop(element, (paths) => {
            setAttachments((current) => mergePaths(current, paths));
            onError(null);
            window.requestAnimationFrame(() => editorRef.current?.focus());
        });
    }, [onError, paneRef]);

    /* A chat is focused again once its session is ready, not only when its pane
       appears: a pane opened while the agent was still starting would otherwise
       keep the caret wherever it was. */
    useEffect(() => {
        if (!visible) return;
        const held = document.activeElement;
        if (held?.closest('input, textarea, [contenteditable="true"], [data-browser-pane]') && !paneRef.current?.contains(held)) return;
        if (held?.closest(".chat-picker-menu")) return;
        const frame = window.requestAnimationFrame(() => editorRef.current?.focus());
        return () => window.cancelAnimationFrame(frame);
    }, [connection, paneRef, visible]);

    const slashToken = slashDismissed ? null : slashTokenAt(draft, caret);
    const slashCommands = useMemo(() => {
        if (!slashToken) return [];
        const needle = slashToken.needle.toLowerCase();
        return commands.filter((command) => command.name.toLowerCase().includes(needle)).slice(0, 8);
    }, [commands, slashToken]);
    const selected = Math.min(slashSelection, Math.max(0, slashCommands.length - 1));

    const selectCommand = (command: AcpAvailableCommand) => {
        if (!slashToken) return;
        const spaced = Boolean(command.input?.hint) && !/^\s/.test(draft.slice(caret));
        const written = `/${command.name}${spaced ? " " : ""}`;
        const position = slashToken.start + written.length;
        setDraft(`${draft.slice(0, slashToken.start)}${written}${draft.slice(caret)}`);
        setCaret(position);
        setSlashDismissed(true);
        window.requestAnimationFrame(() => {
            const editor = editorRef.current;
            if (!editor) return;
            editor.focus();
            editor.setSelectionRange(position, position);
        });
    };

    const blocked = changingConfig || changingPermissions || !permissionApplied;
    const drafted = Boolean(draft.trim()) || attachments.length > 0;

    /* Steering aborts the turn in flight, so the shortcut only fires when there
       is exactly one message waiting and no doubt about which one it takes. */
    const canSteerQueued = running && steerable && queuedCount === 1;

    const send = (steerNow = false) => {
        const text = draft.trim();
        if ((!text && attachments.length === 0) || blocked) return;
        if (!onSend(text, attachments, steerNow)) return;
        setDraft("");
        setCaret(0);
        setSlashSelection(0);
        setAttachments([]);
        setSlashDismissed(false);
    };

    const chooseFiles = async () => {
        try {
            const selection = await open({ multiple: true, directory: false });
            if (!selection) return;
            setAttachments((current) => mergePaths(current, Array.isArray(selection) ? selection : [selection]));
            window.requestAnimationFrame(() => editorRef.current?.focus());
        } catch (failure) {
            onError(failure instanceof Error ? failure.message : String(failure));
        }
    };

    return (
        <div className="chat-composer">
            {slashCommands.length > 0 && <SlashCommands commands={slashCommands} selected={selected} onSelect={selectCommand} />}
            <div className="chat-field">
                {attachments.length > 0 && (
                    <div className="chat-attachments">
                        {attachments.map((path) => (
                            <ComposerAttachment
                                key={path}
                                path={path}
                                onRemove={() => setAttachments((current) => current.filter((candidate) => candidate !== path))}
                            />
                        ))}
                    </div>
                )}
                <textarea
                    ref={editorRef}
                    value={draft}
                    aria-label="Message agent"
                    placeholder={placeholder}
                    rows={2}
                    onChange={(event) => {
                        setDraft(event.target.value);
                        setCaret(event.target.selectionStart);
                        setSlashSelection(0);
                        setSlashDismissed(false);
                        onError(null);
                    }}
                    onSelect={(event) => setCaret(event.currentTarget.selectionStart)}
                    onPaste={(event) => {
                        if (imagesInClipboard(event.clipboardData).length > 0) event.preventDefault();
                        void savePastedClipboard(event.clipboardData)
                            .then((paths) => {
                                if (paths.length === 0) return;
                                setAttachments((current) => mergePaths(current, paths));
                                onError(null);
                            })
                            .catch((failure) => onError(failure instanceof Error ? failure.message : String(failure)));
                    }}
                    onKeyDown={(event) => {
                        if (slashCommands.length > 0) {
                            if (event.key === "ArrowDown") {
                                event.preventDefault();
                                setSlashSelection((current) => (current + 1) % slashCommands.length);
                                return;
                            }
                            if (event.key === "ArrowUp") {
                                event.preventDefault();
                                setSlashSelection((current) => (current - 1 + slashCommands.length) % slashCommands.length);
                                return;
                            }
                            if (event.key === "Tab" || event.key === "Enter") {
                                event.preventDefault();
                                selectCommand(slashCommands[selected]);
                                return;
                            }
                            if (event.key === "Escape") {
                                event.preventDefault();
                                setSlashDismissed(true);
                                return;
                            }
                        }
                        if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
                            event.preventDefault();
                            if (hasPrimaryModifier(event.nativeEvent) && !draft.trim() && attachments.length === 0 && canSteerQueued) {
                                onSteerQueued();
                                return;
                            }
                            send(hasPrimaryModifier(event.nativeEvent));
                        }
                    }}
                />
            </div>
            {error && <div className="chat-composer-error">{error}</div>}
            <div className="chat-composer-bar">
                <button type="button" className="chat-composer-icon" aria-label="Add files" onClick={() => void chooseFiles()}>
                    <IconPlus size={17} />
                </button>
                <YoloToggle
                    agent={agent}
                    relaunches={false}
                    disabled={connection !== "ready" || changingConfig || running || awaitingPermission || changingPermissions || !permissionApplied}
                />
                <ComposerPickers
                    agent={agent}
                    profile={profile}
                    setup={setup}
                    agentLocked={agentLocked}
                    disabled={connection !== "ready" || running || changingPermissions || changingConfig || awaitingPermission}
                    onAgent={(type, profileId) => {
                        if (agentLocked || running || awaitingPermission) return;
                        cmd.configureEmptyAgent(agent.id, type, profileId);
                    }}
                    onConfig={onConfig}
                />
                <ContextMeter usage={usage} agent={agent.type} />
                <span className="chat-composer-spacer" />
                {running && !drafted ? (
                    <button type="button" className="chat-send stop" aria-label="Stop agent" onClick={onStop}>
                        <span />
                    </button>
                ) : (
                    <button
                        type="button"
                        className="chat-send"
                        aria-label="Send message"
                        title={
                            canSteerQueued
                                ? `${PRIMARY_SHORTCUT}↵ steers the queued message into this turn`
                                : running && steerable
                                  ? `Queues behind this turn — ${PRIMARY_SHORTCUT}↵ steers into it`
                                  : undefined
                        }
                        disabled={blocked || !drafted}
                        onClick={() => send()}>
                        <IconArrowUp size={15} />
                    </button>
                )}
            </div>
        </div>
    );
}

export function AgentChatPane({
    agent,
    profile,
    cwd,
    active,
    visible = active,
    onBusyChange,
}: {
    agent: Agent;
    profile?: ProviderProfile;
    cwd: string;
    active: boolean;
    visible?: boolean;
    onBusyChange: (busy: boolean) => void;
}) {
    const home = useStore((s) => s.home);
    const [state, dispatch] = useReducer(chatReducer, initialChatState);
    const [foldMemory] = useState(newFoldMemory);
    const displayStateRef = useRef(state);
    if (visible) displayStateRef.current = state;
    const displayState = displayStateRef.current;
    const [queued, setQueued] = useState<QueuedMessage[]>([]);
    const queuedCount = useRef(0);
    const [composerError, setComposerError] = useState<string | null>(null);
    const [replyingPermission, setReplyingPermission] = useState<string | null>(null);
    const [stoppingTasks, setStoppingTasks] = useState<string[]>([]);
    const [atBottom, setAtBottom] = useState(true);
    const [restartKey, setRestartKey] = useState(0);
    const [reconnectAttempt, setReconnectAttempt] = useState(0);
    const paneRef = useRef<HTMLDivElement>(null);
    const scrollRef = useRef<HTMLDivElement>(null);
    const scrollContentRef = useRef<HTMLDivElement>(null);
    const stickToBottomRef = useRef(true);
    const lastScrollTopRef = useRef(0);
    const lastGestureRef = useRef(0);
    const queuedUpdatesRef = useRef<[string, Record<string, unknown>][]>([]);
    const updateFrameRef = useRef<number | null>(null);
    const updateTimerRef = useRef<number | null>(null);
    const agentRef = useRef(agent);
    agentRef.current = agent;
    const agentLockedRef = useRef(false);
    if (state.messages.length > 0) agentLockedRef.current = true;
    const sessionIdRef = useRef<string | null>(null);
    const lifecycleRef = useRef<Promise<unknown>>(Promise.resolve());
    const [changingConfig, setChangingConfig] = useState(false);
    const configPending = useRef(false);
    const [changingPermissions, setChangingPermissions] = useState(false);
    const [appliedPermissionMode, setAppliedPermissionMode] = useState<string | null>(null);
    const environmentKeys = JSON.stringify(profile?.environmentKeys ?? []);
    const permissionMode = agent.permissionMode ?? (agent.skipPermissions ? "bypass" : "workspace-write");

    /* A restored transcript opens on estimated row heights, and every row that
       measures taller or shorter than the estimate moves the bottom. Anchoring
       to the end makes the list hold the bottom still while that settles. */
    const virtualizer = useVirtualizer({
        count: displayState.messages.length,
        getScrollElement: () => scrollRef.current,
        estimateSize: () => 76,
        overscan: 8,
        anchorTo: "end",
        scrollEndThreshold: BOTTOM_SLACK,
        getItemKey: (index) => displayState.messages[index]?.id ?? index,
    });

    useEffect(() => {
        if (!active) return;
        const backendState =
            state.connection === "error" || state.connection === "stopped"
                ? "stopped"
                : state.permissions.length > 0
                  ? "blocked"
                  : state.running
                    ? "working"
                    : state.connection === "ready"
                      ? "idle"
                      : "unknown";
        cmd.noteAcpAgentState(agent.id, backendState);
    }, [active, agent.id, state.connection, state.running, state.permissions.length]);

    /* A turn ends long before the work it started does. Shells, monitors and
       subagents keep going after the answer, and they die with the agent, so
       what is still running is what says the agent is still in use. */
    const liveTasks = useMemo(() => state.tasks.filter((task) => task.state === "running").length, [state.tasks]);
    const liveSubagents = useMemo(() => runningSubagents(state.messages).length, [state.messages]);
    useEffect(() => cmd.noteAgentBackgroundWork(agent.id, liveTasks, liveSubagents), [agent.id, liveTasks, liveSubagents]);
    useEffect(() => () => cmd.noteAgentBackgroundWork(agent.id, 0, 0), [agent.id]);

    useEffect(() => onBusyChange(state.running), [onBusyChange, state.running]);

    useEffect(() => setQueued([]), [agent.id, cwd]);

    /* A session drops when its adapter exits — a rate limit, a crash, a laptop
       waking up. It resumes itself so the conversation is there to carry on
       with, and only asks once the waits have run out. */
    useEffect(() => {
        if (state.connection === "ready") setReconnectAttempt(0);
    }, [state.connection]);

    useEffect(() => {
        const dropped = state.connection === "error" || state.connection === "stopped";
        if (!active || !dropped || reconnectAttempt >= RECONNECT_DELAYS.length) return;
        const timer = window.setTimeout(() => {
            setReconnectAttempt((value) => value + 1);
            setRestartKey((value) => value + 1);
        }, RECONNECT_DELAYS[reconnectAttempt]);
        return () => window.clearTimeout(timer);
    }, [active, reconnectAttempt, state.connection]);

    const reconnect = useCallback(() => {
        setReconnectAttempt(0);
        setRestartKey((value) => value + 1);
    }, []);

    useEffect(() => {
        if (!active) return;
        const controller = new AbortController();
        let mounted = true;
        const hold = Boolean(agentRef.current.resumeId);
        dispatch({ type: "reset", hold });
        if (!hold) {
            foldMemory.streamed.clear();
            foldMemory.expanded.clear();
        }
        setAppliedPermissionMode(null);
        setChangingPermissions(false);
        sessionIdRef.current = null;

        const flushUpdates = () => {
            if (updateFrameRef.current !== null) {
                window.cancelAnimationFrame(updateFrameRef.current);
                updateFrameRef.current = null;
            }
            if (updateTimerRef.current !== null) {
                window.clearTimeout(updateTimerRef.current);
                updateTimerRef.current = null;
            }
            const updates = queuedUpdatesRef.current.splice(0);
            for (const [sessionId, update] of updates) dispatch({ type: "session_update", sessionId, update });
        };

        /* A hidden window gets no animation frames, so a turn that runs behind
           another tab would pile its whole transcript into one flush the moment
           it comes back. A timer keeps it draining. */
        const queueUpdate = (sessionId: string, update: Record<string, unknown>) => {
            queuedUpdatesRef.current.push([sessionId, update]);
            if (document.hidden) {
                if (updateTimerRef.current === null) updateTimerRef.current = window.setTimeout(flushUpdates, HIDDEN_FLUSH_MS);
                return;
            }
            if (updateFrameRef.current === null) updateFrameRef.current = window.requestAnimationFrame(flushUpdates);
        };

        const handleEvent = (event: AcpEvent) => {
            if (!mounted || event.agentId !== agent.id) return;
            if (event.kind !== "session_update") flushUpdates();
            if (event.kind === "status") dispatch({ type: "status", state: statusFromEvent(event) });
            else if (event.kind === "ready") {
                dispatch({
                    type: "ready",
                    capabilities: recordOf(event.payload.capabilities) ?? {},
                    setup: recordOf(event.payload.setup) ?? {},
                });
            } else if (event.kind === "session_update") {
                const batch = Array.isArray(event.payload.updates) ? event.payload.updates : [];
                for (const entry of batch) {
                    const row = recordOf(entry);
                    if (!row) continue;
                    const update = recordOf(row.update);
                    const sessionId = typeof row.sessionId === "string" ? row.sessionId : null;
                    if (update && sessionId) queueUpdate(sessionId, update);
                }
            } else if (event.kind === "turn_started") {
                if (sessionIdRef.current && agentRef.current.resumeId !== sessionIdRef.current) {
                    cmd.attachAgentSession(agent.id, sessionIdRef.current);
                }
                dispatch({ type: "turn_started" });
            } else if (event.kind === "turn_completed") {
                dispatch({
                    type: "turn_completed",
                    stopReason: typeof event.payload.stopReason === "string" ? event.payload.stopReason : undefined,
                });
            } else if (event.kind === "permission_request") {
                const request = permissionRequest(event.payload);
                if (request) dispatch({ type: "permission_requested", request });
            } else if (event.kind === "error") dispatch({ type: "error", message: eventMessage(event) });
        };

        const lifecycle = lifecycleRef.current
            .catch(() => {})
            .then(async () => {
                if (!mounted) return;
                await acpApi.subscribe(handleEvent, controller.signal);
                if (!mounted) return;
                const current = agentRef.current;
                const initialMode = current.permissionMode ?? (current.skipPermissions ? "bypass" : "workspace-write");
                const response = await acpApi.start({
                    agentId: current.id,
                    provider: current.type,
                    cwd,
                    resumeId: current.resumeId,
                    permissionMode: initialMode,
                    configPath: profile?.configPath,
                    executablePath: profile?.executablePath || current.executablePath,
                    model: current.model,
                    effort: current.effort,
                    environmentKeys: JSON.parse(environmentKeys) as string[],
                });
                if (!mounted) return;
                sessionIdRef.current = response.sessionId;
                dispatch({ type: "ready", capabilities: response.capabilities, setup: response.setup });
                setAppliedPermissionMode(initialMode);
            })
            .catch((error: unknown) => {
                if (!controller.signal.aborted && mounted) {
                    dispatch({ type: "error", message: error instanceof Error ? error.message : String(error) });
                }
            });

        lifecycleRef.current = lifecycle;
        return () => {
            mounted = false;
            sessionIdRef.current = null;
            if (updateFrameRef.current !== null) window.cancelAnimationFrame(updateFrameRef.current);
            updateFrameRef.current = null;
            if (updateTimerRef.current !== null) window.clearTimeout(updateTimerRef.current);
            updateTimerRef.current = null;
            queuedUpdatesRef.current = [];
            controller.abort();
            lifecycleRef.current = lifecycle.finally(() => acpApi.stop(agent.id).catch(() => {}));
        };
    }, [
        active,
        agent.id,
        agent.type,
        agent.profileId,
        agent.executablePath,
        cwd,
        profile?.configPath,
        profile?.executablePath,
        environmentKeys,
        restartKey,
        foldMemory,
    ]);

    useEffect(() => {
        const sessionId = sessionIdRef.current;
        if (
            sessionId === null ||
            state.connection !== "ready" ||
            changingPermissions ||
            appliedPermissionMode === null ||
            permissionMode === appliedPermissionMode
        )
            return;
        setChangingPermissions(true);
        void acpApi
            .setPermissionMode(agent.id, permissionMode)
            .then(() => {
                if (sessionIdRef.current === sessionId) setAppliedPermissionMode(permissionMode);
            })
            .catch((error: unknown) => {
                if (sessionIdRef.current !== sessionId) return;
                const currentMode = agentRef.current.permissionMode ?? (agentRef.current.skipPermissions ? "bypass" : "workspace-write");
                if (currentMode === permissionMode)
                    cmd.setAgentPermissionMode(agent.id, appliedPermissionMode as NonNullable<Agent["permissionMode"]>);
                setComposerError(error instanceof Error ? error.message : String(error));
            })
            .finally(() => {
                if (sessionIdRef.current === sessionId) setChangingPermissions(false);
            });
    }, [agent.id, state.connection, permissionMode, appliedPermissionMode, changingPermissions]);

    const setupRef = useRef(state.setup);
    setupRef.current = state.setup;
    const reported = state.usage !== null;
    useEffect(() => {
        const { resumeId, type } = agentRef.current;
        if (state.connection !== "ready" || reported || !resumeId || (type !== "claude" && type !== "codex")) return;
        let current = true;
        void agentApi
            .sessionContext(type, cwd, resumeId, profile?.configPath)
            .then((saved) => {
                if (!current || !saved) return;
                const size = saved.size ?? guessClaudeWindow(setupRef.current, agentRef.current.model);
                dispatch({ type: "saved_usage", usage: { used: saved.used, size } });
            })
            .catch(() => {});
        return () => {
            current = false;
        };
    }, [agent.id, agent.resumeId, cwd, profile?.configPath, state.connection, reported]);

    useEffect(() => {
        if (state.title && state.title !== agent.title) cmd.setAgentTitle(agent.id, state.title);
    }, [agent.id, agent.title, state.title]);

    const promptNow = useCallback(async (text: string, paths: string[]) => {
        dispatch({ type: "local_prompt", text, paths });
        try {
            await acpApi.prompt(agentRef.current.id, text, paths);
        } catch (error) {
            dispatch({ type: "error", message: error instanceof Error ? error.message : String(error) });
        }
    }, []);

    /* A message written mid-turn waits: it goes out as a prompt of its own once
       the running turn ends, so nothing in flight is cut short. */
    useEffect(() => {
        if (state.connection !== "ready" || state.running || queued.length === 0) return;
        const next = queued[0];
        setQueued((current) => current.filter((message) => message.id !== next.id));
        void promptNow(next.text, next.paths);
    }, [promptNow, queued, state.connection, state.running]);

    /* The scroller's own bottom, not the last message's — a permission card or
       an error sits below the list and still has to be reachable. Idempotent,
       so the observer below can call it until the heights stop moving. */
    const pinToBottom = useCallback(() => {
        const element = scrollRef.current;
        if (!element) return;
        const target = element.scrollHeight - element.clientHeight;
        if (Math.abs(element.scrollTop - target) < 1) return;
        element.scrollTop = target;
        lastScrollTopRef.current = element.scrollTop;
    }, []);

    const noteGesture = useCallback(() => {
        lastGestureRef.current = performance.now();
    }, []);

    /*
     * A restored session opens on estimated row heights. Landing at the
     * estimated bottom mounts the real rows, they measure taller, and the
     * bottom moves again — so one scroll after the messages arrive stops
     * short. Watching the content's height instead re-pins through every
     * settling pass, and through markdown and highlighting that arrive late.
     */
    useLayoutEffect(() => {
        const content = scrollContentRef.current;
        if (!content || typeof ResizeObserver === "undefined") return;
        const observer = new ResizeObserver(() => {
            if (stickToBottomRef.current) pinToBottom();
        });
        observer.observe(content);
        return () => observer.disconnect();
    }, [pinToBottom]);

    useLayoutEffect(() => {
        if (!visible || !stickToBottomRef.current || displayState.messages.length === 0) return;
        pinToBottom();
    }, [displayState.messages.length, displayState.revision, pinToBottom, visible]);

    const steerable = state.capabilities.steering === true;

    /* A turn the agent started on its own may end without the report that
       closes it, so stopping one ends it here too. */
    const stop = () => {
        const unprompted = state.unprompted;
        void acpApi
            .cancel(agent.id)
            .then(() => {
                if (unprompted) dispatch({ type: "turn_completed", stopReason: "cancelled" });
            })
            .catch((failure: unknown) => setComposerError(failure instanceof Error ? failure.message : String(failure)));
    };

    /* Steering stops whatever the agent has in flight so it reads this message
       now, so a message only goes this way when it is asked to. */
    const steer = async (message: QueuedMessage) => {
        setQueued((current) => current.filter((candidate) => candidate.id !== message.id));
        dispatch({ type: "local_prompt", text: message.text, paths: message.paths });
        try {
            if ((await acpApi.steer(agent.id, message.text, message.paths)) !== "promptRequired") return;
            await acpApi.prompt(agent.id, message.text, message.paths);
        } catch (error) {
            dispatch({ type: "error", message: error instanceof Error ? error.message : String(error) });
        }
    };

    /** Says whether the composer may clear what it just handed over. */
    const send = (text: string, paths: string[], steerNow: boolean): boolean => {
        const commandName = text.match(/^\/([^\s]+)/)?.[1];
        if (commandName && state.commands.length > 0 && !state.commands.some((command) => command.name === commandName)) {
            setComposerError(`/${commandName} is not available in this session`);
            return false;
        }
        setComposerError(null);
        if (state.connection === "ready" && !state.running) {
            void promptNow(text, paths);
            return true;
        }

        /* Written mid-turn, or while the session is still coming up: it waits
           in the queue and goes out as its own prompt once the session is free. */
        queuedCount.current += 1;
        const message: QueuedMessage = { id: `queued-${queuedCount.current}`, text, paths };
        if (steerNow && steerable && state.running) {
            void steer(message);
            return true;
        }
        setQueued((current) => [...current, message]);
        return true;
    };

    const stopTask = async (taskId: string) => {
        setStoppingTasks((current) => [...current, taskId]);
        try {
            await acpApi.stopTask(agent.id, taskId);
        } catch (error) {
            setComposerError(error instanceof Error ? error.message : String(error));
        } finally {
            setStoppingTasks((current) => current.filter((candidate) => candidate !== taskId));
        }
    };

    const replyPermission = async (requestId: string, optionId?: string) => {
        setReplyingPermission(requestId);
        try {
            await acpApi.permissionReply(agent.id, requestId, optionId);
            dispatch({ type: "permission_cleared", requestId });
        } catch (error) {
            setComposerError(error instanceof Error ? error.message : String(error));
        } finally {
            setReplyingPermission(null);
        }
    };

    const changeConfig = async (config: SessionConfig, value: string) => {
        if (configPending.current || state.running || changingPermissions) return;
        configPending.current = true;
        setChangingConfig(true);
        setComposerError(null);
        const sessionId = sessionIdRef.current;
        try {
            const response = await acpApi.setConfig(agent.id, config.id, value);
            if (sessionIdRef.current !== sessionId) return;
            dispatch({ type: "config", options: response.configOptions });
            const options = sessionConfigs({ configOptions: response.configOptions });
            const model = options.find((option) => option.id === "model")?.currentValue ?? agent.model;
            const effort =
                options.find((option) => option.id === (agent.type === "claude" ? "effort" : "reasoning_effort"))?.currentValue ?? agent.effort;
            const knownEffort = ["off", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"].includes(effort ?? "")
                ? (effort as Agent["effort"])
                : undefined;
            cmd.setAgentModelPreferences(agent.id, model, knownEffort);
        } catch (error) {
            if (sessionIdRef.current === sessionId) setComposerError(error instanceof Error ? error.message : String(error));
        } finally {
            configPending.current = false;
            setChangingConfig(false);
        }
    };
    const activeTool = useMemo(() => {
        const parts = displayState.messages.at(-1)?.parts ?? [];
        for (let index = parts.length - 1; index >= 0; index -= 1) {
            const part = parts[index];
            if (part.kind !== "tool") continue;
            return toolRunning(part.tool) ? activityLabel(part.tool) : null;
        }
        return null;
    }, [displayState.messages]);
    const subagents = useMemo(() => runningSubagents(displayState.messages), [displayState.messages]);
    const plan = useMemo(() => (displayState.plan === null ? null : formatDetail(displayState.plan)), [displayState.plan]);
    const connecting = connectingLabel(displayState.connection);
    /* A permission card already says what the turn is waiting on, so a spinner
       beside it would only compete with it. */
    const activity =
        displayState.permissions.length > 0
            ? null
            : displayState.running
              ? (activeTool ?? "Thinking…")
              : displayState.messages.length > 0
                ? connecting
                : null;
    const disconnected = displayState.connection === "error" || displayState.connection === "stopped";
    const reconnecting = disconnected && reconnectAttempt < RECONNECT_DELAYS.length;
    const startNewChat = () =>
        cmd.addAgent(agent.type, undefined, undefined, {
            permissionMode: agent.permissionMode,
            profileId: agent.profileId,
            detectedExecutablePath: profile?.executablePath || agent.executablePath,
            cwd,
        });
    const chatAgent = useMemo(() => ({ id: agent.id, type: agent.type }), [agent.id, agent.type]);
    const composerPlaceholder =
        state.connection === "ready"
            ? state.running
                ? "Send to queue behind the running turn"
                : "Ask about this project, or type / for commands"
            : reconnecting
              ? "Reconnecting — this message sends as soon as the session is back"
              : disconnected
                ? "Reconnect to continue this conversation"
                : state.connection === "installing"
                  ? "Installing structured-session adapter…"
                  : state.connection === "starting"
                    ? "Starting agent adapter…"
                    : "Connecting to agent session…";

    return (
        <PathRootsProvider cwd={cwd} home={home}>
            <ChatAgentContext.Provider value={chatAgent}>
                <div className="agent-chat-pane" ref={paneRef}>
                    <div
                        className="chat-scroll"
                        ref={scrollRef}
                        onWheel={noteGesture}
                        onTouchMove={noteGesture}
                        onMouseDown={noteGesture}
                        onKeyDown={noteGesture}
                        onScroll={(event) => {
                            const element = event.currentTarget;
                            const previous = lastScrollTopRef.current;
                            lastScrollTopRef.current = element.scrollTop;
                            const distance = element.scrollHeight - element.scrollTop - element.clientHeight;
                            // The transcript also scrolls itself, to hold the bottom
                            // still while rows settle into their real heights. Only a
                            // scroll up that a wheel, key or drag just asked for means
                            // the reader walked away; sitting at the bottom means stuck.
                            const gesture = lastGestureRef.current;
                            lastGestureRef.current = 0;
                            const walkedAway = element.scrollTop < previous - 1 && performance.now() - gesture < 150;
                            const next = walkedAway ? false : distance < BOTTOM_SLACK ? true : stickToBottomRef.current;
                            if (next === stickToBottomRef.current) return;
                            stickToBottomRef.current = next;
                            setAtBottom(next);
                        }}>
                        <div className="chat-scroll-content" ref={scrollContentRef}>
                            {displayState.messages.length === 0 && (
                                <div className={`chat-connection-state ${displayState.connection}`} role="status">
                                    {(connecting || reconnecting) && <span className="chat-activity-loader" aria-hidden="true" />}
                                    <span>
                                        {reconnecting
                                            ? "Reconnecting…"
                                            : (connecting ??
                                              (displayState.connection === "ready"
                                                  ? "Start a session with this project."
                                                  : displayState.connection === "error"
                                                    ? "Structured session unavailable."
                                                    : "Agent session stopped."))}
                                    </span>
                                    {disconnected && !reconnecting && (
                                        <div className="chat-connection-actions">
                                            <button type="button" onClick={reconnect}>
                                                Reconnect
                                            </button>
                                            {agent.resumeId && (
                                                <button type="button" onClick={startNewChat}>
                                                    Start new chat
                                                </button>
                                            )}
                                        </div>
                                    )}
                                </div>
                            )}
                            <FoldMemoryContext value={foldMemory}>
                                <div className="chat-virtual-space" style={{ height: `${virtualizer.getTotalSize()}px` }}>
                                    {virtualizer.getVirtualItems().map((item) => {
                                        const message = displayState.messages[item.index];
                                        const meta = rowMeta(displayState.messages, item.index);
                                        return (
                                            <div
                                                key={message.id}
                                                data-index={item.index}
                                                ref={virtualizer.measureElement}
                                                className="chat-virtual-row"
                                                style={{ transform: `translateY(${item.start}px)` }}>
                                                <ChatMessageRow
                                                    message={message}
                                                    live={displayState.running && item.index === displayState.messages.length - 1}
                                                    copyable={meta.text}
                                                    rate={meta.rate}
                                                />
                                            </div>
                                        );
                                    })}
                                </div>
                            </FoldMemoryContext>
                            {activity && <ChatActivity key={displayState.running ? "turn" : "connect"} label={activity} agentType={agent.type} />}
                            {plan !== null && (
                                <details className="chat-plan">
                                    <summary>Plan</summary>
                                    <pre>{plan}</pre>
                                </details>
                            )}
                            {displayState.permissions.map((request) => (
                                <PermissionRequest
                                    key={request.requestId}
                                    request={request}
                                    busy={replyingPermission === request.requestId}
                                    onReply={(optionId) => void replyPermission(request.requestId, optionId)}
                                />
                            ))}
                            {displayState.error && (
                                <div className="chat-error" role="alert">
                                    <IconWarning size={14} />
                                    <span>{displayState.error}</span>
                                </div>
                            )}
                            {displayState.messages.length > 0 && disconnected && (
                                <div className="chat-reconnect" role="status">
                                    {reconnecting ? <span className="chat-activity-loader" aria-hidden="true" /> : <IconPlug size={13} />}
                                    <span>{reconnecting ? "Reconnecting…" : "This session dropped."}</span>
                                    {!reconnecting && (
                                        <div className="chat-connection-actions">
                                            <button type="button" onClick={reconnect}>
                                                Reconnect
                                            </button>
                                            {agent.resumeId && (
                                                <button type="button" onClick={startNewChat}>
                                                    Start new chat
                                                </button>
                                            )}
                                        </div>
                                    )}
                                </div>
                            )}
                        </div>
                    </div>

                    <div className="chat-composer-wrap">
                        {!atBottom && displayState.messages.length > 0 && (
                            <button
                                type="button"
                                className="chat-jump-bottom"
                                aria-label="Jump to latest message"
                                onClick={() => {
                                    stickToBottomRef.current = true;
                                    setAtBottom(true);
                                    pinToBottom();
                                }}>
                                <IconArrowDown size={14} />
                            </button>
                        )}
                        {(subagents.length > 0 || displayState.tasks.length > 0 || queued.length > 0) && (
                            <div className="chat-live-stack">
                                <RunningSubagents subagents={subagents} />
                                <BackgroundTasks tasks={displayState.tasks} stopping={stoppingTasks} onStop={(taskId) => void stopTask(taskId)} />
                                <QueuedMessages
                                    messages={queued}
                                    steerable={steerable && state.running}
                                    onSteer={(message) => void steer(message)}
                                    onDrop={(id) => setQueued((current) => current.filter((message) => message.id !== id))}
                                />
                            </div>
                        )}
                        <ChatComposer
                            agent={agent}
                            profile={profile}
                            paneRef={paneRef}
                            visible={visible}
                            connection={state.connection}
                            running={state.running}
                            steerable={steerable}
                            commands={state.commands}
                            setup={state.setup}
                            awaitingPermission={state.permissions.length > 0}
                            agentLocked={agentLockedRef.current}
                            changingConfig={changingConfig}
                            changingPermissions={changingPermissions}
                            permissionApplied={state.connection !== "ready" || permissionMode === appliedPermissionMode}
                            placeholder={composerPlaceholder}
                            error={composerError}
                            onError={setComposerError}
                            onSend={send}
                            onSteerQueued={() => {
                                const head = queued[0];
                                if (head) void steer(head);
                            }}
                            onStop={stop}
                            queuedCount={queued.length}
                            usage={state.usage}
                            onConfig={changeConfig}
                        />
                    </div>
                    <div className="chat-drop-target" aria-hidden="true">
                        <IconFile size={22} />
                        <span>Drop files or folders into this session</span>
                    </div>
                </div>
            </ChatAgentContext.Provider>
        </PathRootsProvider>
    );
}
