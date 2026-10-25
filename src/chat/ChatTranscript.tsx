import { useEffect, useMemo, useState, type ReactNode, type RefObject, type UIEvent } from "react";
import type { Agent } from "../state/types";
import { ChatActivity } from "./ChatActivity";
import { ChatMessageRow } from "./ChatMessageRow";
import { FoldMemoryContext, type FoldMemory } from "@shared/chat/longText";
import { copyText } from "@shared/chat/messageMeta";
import { PermissionRequest } from "./PermissionRequest";
import { formatDetail, outlivesFold } from "@shared/chat/transcript";
import type { PermissionAsk, ChatMessage, ChatRows } from "@shared/chat/types";
import { WorkSummary } from "./WorkSummary";

/* A long transcript mounts its newest rows first and the older ones above
   them a slice at a time. Laid out from the bottom, rows added above the view
   never move it. */
const FIRST_ROWS = 40;

/** Said above a chat whose host no longer keeps its first turns. */
export const EARLIER_LOST = "Earlier messages are no longer kept.";

/** Where each finished turn that folds starts and ends among the rows, by the index of each row in it. */
function foldSpans(messages: readonly ChatMessage[]): Map<number, { start: number; end: number }> {
    const spans = new Map<number, { start: number; end: number }>();
    let start = -1;
    for (let index = 0; index < messages.length; index += 1) {
        const fold = messages[index].fold;
        if (!fold) continue;
        if (fold.id === messages[index].id || start < 0) start = index;
        if (fold.last !== messages[index].id) continue;
        for (let at = start; at <= index; at += 1) spans.set(at, { start, end: index });
        start = -1;
    }
    return spans;
}
const ROWS_PER_SLICE = 60;

function useOlderRows(count: number): number {
    const [start, setStart] = useState(0);
    const [opened, setOpened] = useState(false);
    if (!opened && count > 0) {
        setOpened(true);
        setStart(Math.max(0, count - FIRST_ROWS));
    } else if (opened && count === 0) {
        setOpened(false);
        setStart(0);
    }
    useEffect(() => {
        if (start === 0) return;
        const timer = window.setTimeout(() => setStart((current) => Math.max(0, current - ROWS_PER_SLICE)), 16);
        return () => window.clearTimeout(timer);
    }, [start]);
    return Math.min(start, count);
}

export interface TranscriptPermissions {
    requests: readonly PermissionAsk[];
    /** The request whose answer is on its way. */
    busy: string | null;
    /** Without it the requests are shown but cannot be answered. */
    onReply?: (requestId: string, optionId?: string) => void;
}

/**
 * The scrolling part of a chat: its messages, folded into work summaries,
 * what the agent is doing now, its plan and what it waits on. The agent's
 * pane and a shared chat draw it the same way.
 */
export function ChatTranscript({
    state,
    agentType,
    foldMemory,
    unfolded,
    scrollRef,
    contentRef,
    onScroll,
    activity,
    permissions,
    onEdit,
    before,
    after,
}: {
    state: ChatRows;
    agentType: Agent["type"];
    foldMemory: FoldMemory;
    /** Shows every fold open, as while finding in the chat. */
    unfolded: boolean;
    scrollRef: RefObject<HTMLDivElement | null>;
    contentRef: RefObject<HTMLDivElement | null>;
    onScroll: (event: UIEvent<HTMLDivElement>) => void;
    activity: string | null;
    permissions: TranscriptPermissions;
    onEdit?: (message: ChatMessage, text: string) => void;
    /** Drawn above the messages. */
    before?: ReactNode;
    /** Drawn below everything else. */
    after?: ReactNode;
}) {
    const firstRow = useOlderRows(state.messages.length);
    const spans = useMemo(() => foldSpans(state.messages), [state.messages]);
    const [openWork, setOpenWork] = useState<ReadonlySet<string>>(() => new Set());
    const toggleWork = (id: string) =>
        setOpenWork((open) => {
            const next = new Set(open);
            if (!next.delete(id)) next.add(id);
            return next;
        });
    const plan = useMemo(() => (state.plan === undefined ? null : formatDetail(state.plan)), [state.plan]);

    return (
        <div className="chat-scroll" ref={scrollRef} onScroll={onScroll}>
            <div className="chat-scroll-content" ref={contentRef}>
                {state.earlierLost && (
                    <div className="chat-earlier" role="status">
                        {EARLIER_LOST}
                    </div>
                )}
                {before}
                <FoldMemoryContext value={foldMemory}>
                    {state.messages.slice(firstRow).map((message, offset) => {
                        const index = firstRow + offset;
                        const meta = message.meta;
                        const span = spans.get(index);
                        const fold = span && message.fold;
                        const workId = fold ? fold.id : "";
                        const folded = span !== undefined && !unfolded && !openWork.has(workId);
                        const summary = span && fold && index === Math.max(span.start, firstRow) && (
                            <WorkSummary
                                agent={agentType}
                                took={fold.took ?? null}
                                calls={fold.calls}
                                thought={fold.thought}
                                open={!folded}
                                onToggle={() => toggleWork(workId)}
                            />
                        );
                        if (span && folded && index < span.end && !message.parts.some(outlivesFold))
                            return (
                                <div key={message.id} data-index={index} className="chat-row">
                                    {summary}
                                </div>
                            );
                        return (
                            <div key={message.id} data-index={index} className="chat-row">
                                {summary}
                                <ChatMessageRow
                                    from={span && fold && folded ? (index < span.end ? message.parts.length : fold.from) : 0}
                                    message={message}
                                    live={state.running && index === state.messages.length - 1}
                                    copyable={copyText(state.messages, index)}
                                    rate={meta?.rate ?? null}
                                    at={message.role === "user" ? (message.sentAt ?? null) : (meta?.at ?? null)}
                                    took={meta?.took ?? null}
                                    onEdit={onEdit && message.role === "user" && message.promptId ? onEdit : undefined}
                                />
                            </div>
                        );
                    })}
                </FoldMemoryContext>
                {activity &&
                    (state.running ? (
                        <ChatActivity label={activity} agentType={agentType} />
                    ) : (
                        <div className="chat-activity" role="status">
                            <span className="chat-activity-loader" aria-hidden="true" />
                            <span className="chat-activity-label">{activity}</span>
                        </div>
                    ))}
                {plan !== null && (
                    <details className="chat-plan">
                        <summary>Plan</summary>
                        <pre>{plan}</pre>
                    </details>
                )}
                {permissions.requests.map((request) => (
                    <PermissionRequest
                        key={request.requestId}
                        request={request}
                        busy={permissions.busy === request.requestId}
                        onReply={permissions.onReply ? (optionId) => permissions.onReply?.(request.requestId, optionId) : undefined}
                    />
                ))}
                {after}
            </div>
        </div>
    );
}
