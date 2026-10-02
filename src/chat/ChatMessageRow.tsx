import { memo, useContext, useMemo, useState } from "react";
import { CopyButton } from "../ui/CopyButton";
import { basename } from "../lib/paths";
import { AgentIcon, IconCheck, IconChevron, IconClose, IconFile, IconPlug, IconTimer, IconWarning } from "../ui/Icons";
import { rateLabel, sentLabel, sentTitle } from "./messageMeta";
import { durationLabel } from "./durationLabel";
import { localImagePath, useImagePreview } from "./imagePreview";
import { ChatFileRef, useFileRef } from "./FileRef";
import { ChatAgentContext } from "./chatAgent";
import { ChatImage } from "./ChatImage";
import { FoldedMarkdown } from "./ChatMarkdown";
import { ToolGroup } from "./ToolGroup";
import { ToolRow } from "./ToolRow";
import { attachmentName, formatDetail, groupParts, subagentTask } from "./transcript";
import type { AcpSubagent, AcpTaskNotice, ChatMessage, ChatPart } from "./types";
import { ContextChipLabel } from "./ContextChip";
import { splitSentContext, type SentContext } from "./promptContext";

function embeddedContext(content: Extract<ChatPart, { kind: "content" }>["content"]): SentContext | null {
    if (content.type !== "resource" || !content.resource || typeof content.resource !== "object") return null;
    const resource = content.resource as { uri?: unknown; text?: unknown };
    if (typeof resource.uri !== "string") return null;
    const firstLine = typeof resource.text === "string" ? resource.text.split("\n", 1)[0] : "";
    return { uri: resource.uri, title: firstLine || resource.uri };
}

/* What a person sent with a message is shown by name: their own text stays
   prose, and an issue they handed over stays a chip, not its whole text. */
function sentParts(message: ChatMessage): { parts: ChatPart[]; context: SentContext[] } {
    const context = [...(message.context ?? [])];
    if (message.role !== "user") return { parts: message.parts, context };
    const parts: ChatPart[] = [];
    for (const part of message.parts) {
        const embedded = part.kind === "content" ? embeddedContext(part.content) : null;
        if (embedded) {
            context.push(embedded);
            continue;
        }
        if (part.kind !== "text") {
            parts.push(part);
            continue;
        }
        const split = splitSentContext(part.text);
        context.push(...split.context);
        if (split.text) parts.push(split.context.length ? { ...part, text: split.text } : part);
    }
    return { parts, context: context.filter((item, index) => context.findIndex((other) => other.uri === item.uri) === index) };
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
    if (file) return <ChatFileRef refers={file.ref} state={file.state} label={basename(path)} size={34} className="chat-attachment-file" tile />;
    return (
        <span title={path}>
            <IconFile size={12} />
            {basename(path)}
        </span>
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
            <span className="chat-notice-name" title={notice.name}>
                {notice.name}
            </span>
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
                <span className="chat-subagent-task">{subagentTask(subagent.task)}</span>
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

export const ChatMessageRow = memo(function ChatMessageRow({
    message,
    live,
    copyable,
    rate,
    at,
    took,
}: {
    message: ChatMessage;
    live: boolean;
    copyable: string;
    rate: number | null;
    at: number | null;
    took: number | null;
}) {
    const { parts, context } = useMemo(() => sentParts(message), [message]);
    const attachments = message.attachments ?? [];
    return (
        <article className={`chat-message ${message.role}`}>
            <div className="chat-message-content">
                {(attachments.length > 0 || context.length > 0) && (
                    <div className="chat-message-attachments">
                        {attachments.map((path) => (
                            <SentAttachment key={path} path={path} />
                        ))}
                        {context.map((item) => (
                            <span key={item.uri} className="chat-context-chip" title={item.uri}>
                                <ContextChipLabel item={item} />
                            </span>
                        ))}
                    </div>
                )}
                <PartGroups parts={parts} live={live} typed={message.role === "user"} />
                {copyable && (
                    <div className="chat-message-meta">
                        <CopyButton value={copyable} label={message.role === "user" ? "message" : "reply"} size={15} />
                        {at !== null && (
                            <time className="chat-message-time" dateTime={new Date(at).toISOString()} title={sentTitle(at)}>
                                {took === null ? sentLabel(at) : `${sentLabel(at)} · took ${durationLabel(took)}`}
                            </time>
                        )}
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
