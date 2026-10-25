import { memo, useContext, useMemo, useState, type ReactNode } from "react";
import { CopyButton } from "../ui/CopyButton";
import { basename } from "@shared/lib/paths";
import {
    AgentIcon,
    IconCheck,
    IconChevron,
    IconClose,
    IconFile,
    IconInfo,
    IconPencil,
    IconPlug,
    IconTimer,
    IconUser,
    IconWarning,
    IconWorkflow,
} from "@shared/ui/Icons";
import { Tooltip } from "../ui/Tooltip";
import { AgentMark } from "../agents/AgentMark";
import sikemuxMark from "../../brand/mark/mark-purple.svg";
import * as cmd from "../state/commands";
import { AnsweredQuestion } from "./AgentQuestions";
import { rateLabel, sentLabel, sentTitle } from "@shared/chat/messageMeta";
import { durationLabel } from "@shared/chat/durationLabel";
import { localImagePath } from "@shared/lib/imagePaths";
import { useImagePreview } from "./imagePreview";
import { ChatFileRef, useFileRef } from "./FileRef";
import { ChatAgentContext, ChatSubagentsContext } from "./chatAgent";
import { ChatImage } from "./ChatImage";
import { imageTag } from "@shared/chat/composerInput";
import { FoldedMarkdown, warmChatMarkdown } from "./ChatMarkdown";
import { ChatPage } from "./ChatPage";
import { ToolGroup } from "./ToolGroup";
import { ToolRow } from "./ToolRow";
import {
    attachmentName,
    formatDetail,
    groupParts,
    shownParts,
    SUBAGENT_MARKS,
    SUBAGENT_WORDS,
    subagentCost,
    subagentTask,
    type SubagentMark,
} from "@shared/chat/transcript";
import type { AccountMove, AgentLetter, AgentNotice, ChatMessage, ChatPart, ContentBlock, SubagentCard, TaskNotice } from "@shared/chat/types";
import { ContextChipLabel } from "./ContextChip";
import type { MdElement } from "@shared/markdown/types";
import { Shimmer } from "./Shimmer";
import { MessageEditor } from "./MessageEditor";

function ResourceLinkPart({ content }: { content: ContentBlock }) {
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
    const content = part.content as ContentBlock;
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

const warmed = new WeakSet<ChatMessage>();
let warming: Promise<void> = Promise.resolve();

/* A row draws its text in the frame it mounts only if that text was read
   already; otherwise it mounts empty and grows a frame later, under the reader.
   Messages are read newest first, since reading starts at the bottom; the last
   one may still be growing, and the row showing it reads it anyway. */
export function warmTranscript(messages: readonly ChatMessage[]): Promise<void> {
    const texts: { text: string; typed: boolean }[] = [];
    for (let index = messages.length - 2; index >= 0; index -= 1) {
        const message = messages[index];
        if (warmed.has(message)) continue;
        warmed.add(message);
        if (message.letters?.length) {
            texts.push(...message.letters.map((letter) => ({ text: letter.text, typed: false })));
            continue;
        }
        for (const part of message.parts) {
            if (part.kind === "text" && !part.md) texts.push({ text: part.text, typed: message.role === "user" });
            else if (part.kind === "thought" && !part.md) texts.push({ text: part.text, typed: false });
        }
    }
    /* Settles once everything asked for so far has been read, by this call or an earlier one. */
    if (texts.length > 0) warming = Promise.all([warming, warmChatMarkdown(texts)]).then(() => undefined);
    return warming;
}

const MessagePart = memo(function MessagePart({ part, live, typed }: { part: ChatPart; live: boolean; typed: boolean }) {
    if (part.kind === "text") {
        return (
            <div className="chat-markdown">
                <FoldedMarkdown id={part.id} text={part.text} live={live} typed={typed} tree={part.md?.blocks as MdElement[] | undefined} />
            </div>
        );
    }
    if (part.kind === "thought") {
        return (
            <div className="chat-thought">
                <div className="chat-markdown">
                    <FoldedMarkdown id={part.id} text={part.text} live={live} tree={part.md?.blocks as MdElement[] | undefined} />
                </div>
            </div>
        );
    }
    if (part.kind === "tool") return part.page ? <ChatPage page={part.page} /> : <ToolRow part={part} />;
    if (part.kind === "subagent") return <SubagentPart subagent={part.subagent} />;
    if (part.kind === "notice") return <NoticePart notice={part.notice} />;
    if (part.kind === "agent_notice") return <AgentNoticePart notice={part.notice} />;
    if (part.kind === "account") return <AccountPart move={part.move} />;
    return <ContentPart part={part} />;
});

function SentAttachment({ path, number }: { path: string; number: number | null }) {
    const preview = useImagePreview(path);
    const file = useFileRef(path);
    if (preview && number !== null)
        return (
            <div className="chat-attachment-tagged">
                <ChatImage src={preview} path={path} className="chat-attachment-thumb" />
                <span className="chat-attachment-number">#{number}</span>
            </div>
        );
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

function NoticePart({ notice }: { notice: TaskNotice }) {
    const subagent = useContext(ChatSubagentsContext).cards.get(notice.subagent ?? "");
    if (subagent) return <SubagentPart subagent={subagent} />;
    return (
        <div className={`chat-notice state-${notice.state}`} role="status">
            {notice.taskType === "workflow" ? <IconWorkflow size={12} /> : <IconTimer size={12} />}
            <span className="chat-notice-name" title={notice.name}>
                {notice.name}
            </span>
            <span className="chat-notice-state">{notice.state}</span>
            {notice.summary && <span className="chat-notice-summary">{notice.summary}</span>}
        </div>
    );
}

function AgentNoticePart({ notice }: { notice: AgentNotice }) {
    return (
        <div className={`chat-notice agent severity-${notice.severity}`} role="status">
            {notice.severity === "info" ? <IconInfo size={12} /> : <IconWarning size={12} />}
            <span className="chat-notice-state">{notice.title}</span>
            {notice.description && (
                <span className="chat-notice-summary" title={notice.description}>
                    {notice.description}
                </span>
            )}
        </div>
    );
}

function AccountPart({ move }: { move: AccountMove }) {
    const why = move.reason === "limit" && move.from ? `${move.from} reached its usage limit` : null;
    return (
        <div className="chat-notice account" role="status">
            <IconUser size={12} />
            <span className="chat-notice-state">Moved to</span>
            <span className="chat-notice-name" title={move.label}>
                {move.label}
            </span>
            {why && <span className="chat-notice-summary">{why}</span>}
        </div>
    );
}

const SUBAGENT_ICONS: Record<Exclude<SubagentMark, "spinner">, ReactNode> = {
    check: <IconCheck size={13} />,
    warning: <IconWarning size={12} />,
    plug: <IconPlug size={12} />,
    close: <IconClose size={11} />,
};

function SubagentStateMark({ state }: { state: SubagentCard["state"] }) {
    const word = SUBAGENT_WORDS[state];
    const mark = SUBAGENT_MARKS[state];
    return (
        <span className="chat-subagent-state" role="img" aria-label={word} title={word}>
            {mark === "spinner" ? <span className="chat-subagent-spinner" aria-hidden="true" /> : SUBAGENT_ICONS[mark]}
        </span>
    );
}

/* A folded subagent keeps streaming into a transcript nobody is reading, so its
   body is only built once the reader opens it. */
function SubagentPart({ subagent }: { subagent: SubagentCard }) {
    const agentType = useContext(ChatAgentContext).type;
    const rows = useContext(ChatSubagentsContext).rows[subagent.sessionId];
    const [open, setOpen] = useState(false);
    const parts = useMemo(() => (rows ?? []).flatMap((message) => message.parts), [rows]);
    const cost = subagentCost(subagent);
    return (
        <details className={`chat-subagent state-${subagent.state}`} open={open} onToggle={(event) => setOpen(event.currentTarget.open)}>
            <summary>
                <IconChevron size={9} className="chat-subagent-chevron" />
                <span className="chat-subagent-mark">
                    <AgentIcon type={agentType} size={18} className={`agent-glyph ${agentType}`} />
                </span>
                <span className="chat-subagent-name">{subagent.name}</span>
                {subagent.state === "running" ? (
                    <span className="chat-subagent-task chat-shimmer">
                        <Shimmer>{subagentTask(subagent.task)}</Shimmer>
                    </span>
                ) : (
                    <span className="chat-subagent-task">{subagentTask(subagent.task)}</span>
                )}
                <span className="chat-subagent-end">
                    {cost && <span className="chat-subagent-calls">{cost}</span>}
                    <SubagentStateMark state={subagent.state} />
                </span>
            </summary>
            {open && (
                <div className="chat-subagent-body">
                    <div>
                        {parts.length > 0 ? (
                            <PartGroups parts={parts} live={subagent.state === "running"} />
                        ) : (
                            <span className="chat-subagent-empty">No output yet.</span>
                        )}
                    </div>
                </div>
            )}
        </details>
    );
}

function SentTime({ at, took }: { at: number; took: number | null }) {
    return (
        <time className="chat-message-time" dateTime={new Date(at).toISOString()} title={sentTitle(at)}>
            {took === null ? sentLabel(at) : `${sentLabel(at)} · took ${durationLabel(took)}`}
        </time>
    );
}

/** A teammate's message, headed like the agent's own "Worked for" line. */
function TeammateLetter({ id, letter }: { id: string; letter: AgentLetter }) {
    return (
        <div className="chat-letter" title={letter.agentId}>
            <div className="chat-letter-top">
                <span className="chat-letter-head">
                    <span className="chat-work-mark" aria-hidden="true">
                        {letter.cli === "sikemux" ? (
                            <img src={sikemuxMark} width={17} height={17} alt="" />
                        ) : (
                            <AgentMark agentId={letter.agentId} cli={letter.cli} size={17} />
                        )}
                    </span>
                    <span>
                        Message from{" "}
                        <button
                            type="button"
                            className="chat-letter-name"
                            title={`Open ${letter.name}`}
                            onClick={() => letter.agentId && cmd.revealAgent(letter.agentId)}>
                            {letter.name}
                        </button>
                    </span>
                </span>
            </div>
            <div className="chat-markdown">
                <FoldedMarkdown id={id} text={letter.text} live={false} tree={letter.tree?.blocks as MdElement[] | undefined} />
            </div>
        </div>
    );
}

function AgentLetters({ id, letters, at }: { id: string; letters: AgentLetter[]; at: number | null }) {
    return (
        <article className="chat-message agent-letter">
            <div className="chat-message-content">
                {letters.map((letter, index) =>
                    letter.kind === "answer" ? (
                        <div key={index} className="chat-letter-answer">
                            <AnsweredQuestion letter={letter} />
                        </div>
                    ) : (
                        <TeammateLetter key={index} id={`${id}-letter-${index}`} letter={letter} />
                    ),
                )}
                <div className="chat-message-meta">
                    <CopyButton value={letters.map((letter) => letter.text).join("\n\n")} label="message" size={15} />
                    {at !== null && <SentTime at={at} took={null} />}
                </div>
            </div>
        </article>
    );
}

export const ChatMessageRow = memo(function ChatMessageRow({
    message,
    live,
    copyable,
    rate,
    at,
    took,
    onEdit,
    from = 0,
}: {
    message: ChatMessage;
    live: boolean;
    copyable: string;
    rate: number | null;
    at: number | null;
    took: number | null;
    /** Given when the person may write this message again. */
    onEdit?: (message: ChatMessage, text: string) => void;
    /** Where the shown parts start, when the work before them is folded away. */
    from?: number;
}) {
    const { parts, letters } = message;
    const context = message.context ?? [];
    const shown = useMemo(() => shownParts(parts, from), [parts, from]);
    const [editing, setEditing] = useState(false);
    const attachments = message.attachments ?? [];
    const written = parts.flatMap((part) => (part.kind === "text" ? [part.text] : [])).join("\n\n");
    if (letters?.length) return <AgentLetters id={message.id} letters={letters} at={at} />;
    if (editing && onEdit) {
        return (
            <article className={`chat-message ${message.role}`}>
                <div className="chat-message-content">
                    <MessageEditor
                        initial={written}
                        onCancel={() => setEditing(false)}
                        onSend={(text) => {
                            setEditing(false);
                            onEdit(message, text);
                        }}
                    />
                </div>
            </article>
        );
    }
    return (
        <article className={`chat-message ${message.role}`}>
            <div className="chat-message-content">
                {(attachments.length > 0 || context.length > 0) && (
                    <div className="chat-message-attachments">
                        {attachments.map((path, index) => (
                            <SentAttachment key={path} path={path} number={written.includes(imageTag(index + 1)) ? index + 1 : null} />
                        ))}
                        {context.map((item) => (
                            <span key={item.uri} className="chat-context-chip" title={item.uri}>
                                <ContextChipLabel item={item} />
                            </span>
                        ))}
                    </div>
                )}
                <PartGroups parts={shown} live={live} typed={message.role === "user"} />
                {copyable && (
                    <div className="chat-message-meta">
                        {onEdit && context.length === 0 && (
                            <Tooltip label="Edit message">
                                <button type="button" className="copy-btn" aria-label="Edit message" onClick={() => setEditing(true)}>
                                    <IconPencil size={15} />
                                </button>
                            </Tooltip>
                        )}
                        <CopyButton value={copyable} label={message.role === "user" ? "message" : "reply"} size={15} />
                        {at !== null && <SentTime at={at} took={took} />}
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
