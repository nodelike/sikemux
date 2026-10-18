import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import { agentLabel, isAgentType } from "../agents/catalog";
import { ChatAgentContext, ChatSubagentsContext, ReaderScrollContext } from "../chat/chatAgent";
import { warmTranscript } from "../chat/ChatMessageRow";
import { ChatTranscript } from "../chat/ChatTranscript";
import { EarlierTurns } from "../chat/EarlierTurns";
import { activityText } from "../chat/chatStatus";
import { PathRootsProvider } from "../chat/FileRef";
import LiveStack from "../chat/LiveStack";
import { newFoldMemory } from "../chat/longText";
import { RemoteOriginContext } from "../chat/origin";
import { activeToolLabel } from "../chat/toolLabels";
import { runningSubagents, subagentsById } from "../chat/transcript";
import { useMessageArrival } from "../chat/useMessageArrival";
import { useStickToBottom } from "../chat/useStickToBottom";
import { AgentIcon, IconArrowDown, IconEye, IconPlug } from "../ui/Icons";
import { sharedChatTitle, type ShareSession, type ShareSnapshot } from "./session";
import "../styles/chat.css";
import "../styles/share.css";

const PROBLEMS: Partial<Record<ShareSnapshot["status"], string>> = {
    missing: "This chat is not running on its host right now.",
    notFound: "No chat is shared at this link.",
    signedOut: "Sign in to Sikemux to watch shared chats.",
    unreadable: "This link does not open the chat it points at. Check that the whole link was copied.",
};

function endedText(snapshot: ShareSnapshot): string {
    return snapshot.endedBecause === "expired" ? "This share has expired." : "Sharing this chat has stopped.";
}

function endsIn(expiresAt: number, now: number): string {
    const minutes = Math.max(1, Math.round((expiresAt - now) / 60_000));
    if (minutes < 60) return `${minutes} min`;
    const hours = Math.round(minutes / 60);
    if (hours < 48) return hours === 1 ? "1 hour" : `${hours} hours`;
    return `${Math.round(hours / 24)} days`;
}

/**
 * Someone's chat, shared read-only: the same header and transcript as the agent's own pane,
 * without a composer, and a line under it saying whose it is and how long it stays open.
 */
export function SharedChat({ session, visible, actions }: { session: ShareSession; visible: boolean; actions?: ReactNode }) {
    const snapshot = useSyncExternalStore(session.subscribe, session.snapshot);
    const { agent: state, chat } = snapshot;
    const [foldMemory] = useState(newFoldMemory);
    const scrollRef = useRef<HTMLDivElement>(null);
    const contentRef = useRef<HTMLDivElement>(null);
    useMessageArrival(scrollRef, state.messages);
    useEffect(() => void warmTranscript(state.messages), [state.messages]);
    const { atBottom, onScroll, jumpToBottom } = useStickToBottom({ scrollRef, contentRef, visible });
    const scrollByReader = useCallback((deltaY: number) => scrollRef.current?.scrollBy({ top: deltaY }), []);
    const [foldedLive, setFoldedLive] = useState<ReadonlySet<string>>(() => new Set());
    const liveFolds = useMemo(
        () => ({
            folded: foldedLive,
            toggle: (kind: string) =>
                setFoldedLive((folded) => {
                    const next = new Set(folded);
                    if (!next.delete(kind)) next.add(kind);
                    return next;
                }),
        }),
        [foldedLive],
    );

    const provider = chat?.provider;
    const type = isAgentType(provider) ? provider : "claude";
    const chatAgent = useMemo(() => ({ id: "", type }), [type]);
    const origin = useMemo(() => ({ readImage: session.readImage }), [session]);
    const shown = useMemo(() => ({ ...state, permissions: [...snapshot.permissions] }), [state, snapshot.permissions]);
    const activeTool = useMemo(() => activeToolLabel(state.messages), [state.messages]);
    const activity = snapshot.status === "live" ? activityText(shown, activeTool) : null;
    const subagents = useMemo(() => runningSubagents(state.messages), [state.messages]);
    const subagentCards = useMemo(() => subagentsById(state.messages), [state.messages]);
    const problem = snapshot.status === "ended" ? endedText(snapshot) : PROBLEMS[snapshot.status];
    const title = sharedChatTitle(snapshot) ?? `${agentLabel(type)} chat`;

    const before = (
        <>
            {snapshot.hasEarlier && <EarlierTurns state={snapshot.earlier} onLoad={session.loadEarlier} scrollRef={scrollRef} />}
            {state.messages.length === 0 && (
                <div className={`chat-connection-state ${problem ? "stopped" : "connecting"}`} role="status">
                    {!problem && <span className="chat-activity-loader" aria-hidden="true" />}
                    <span>{problem ?? "Opening the shared chat…"}</span>
                </div>
            )}
        </>
    );
    const after = state.messages.length > 0 && problem && (
        <div className="chat-reconnect" role="status">
            <IconPlug size={13} />
            <span>{problem}</span>
        </div>
    );

    return (
        <RemoteOriginContext value={origin}>
            <PathRootsProvider cwd={chat?.cwd ?? ""} home={chat?.home ?? undefined}>
                <ChatAgentContext.Provider value={chatAgent}>
                    <ChatSubagentsContext.Provider value={subagentCards}>
                        <ReaderScrollContext.Provider value={scrollByReader}>
                            <section className="agent-surface shared-chat" data-view="gui">
                                <header className="agent-surface-header">
                                    <div className="agent-surface-name">
                                        <span className={`agent-surface-mark agent-glyph ${type}`} aria-hidden="true">
                                            <AgentIcon type={type} size={16} />
                                        </span>
                                        <span className="agent-surface-title" title={title}>
                                            {title}
                                        </span>
                                    </div>
                                    <div className="agent-surface-controls">
                                        <span className="shared-chat-badge">
                                            <IconEye size={13} />
                                            <span>Read-only</span>
                                        </span>
                                        {actions}
                                    </div>
                                </header>
                                <div className="agent-surface-body">
                                    <div className="agent-gui-layer visible">
                                        <div className="agent-chat-pane">
                                            <ChatTranscript
                                                state={shown}
                                                agentType={type}
                                                foldMemory={foldMemory}
                                                unfolded={false}
                                                scrollRef={scrollRef}
                                                contentRef={contentRef}
                                                onScroll={onScroll}
                                                activity={activity}
                                                permissions={{ requests: snapshot.permissions, busy: null }}
                                                before={before}
                                                after={after}
                                            />
                                            <div className="chat-composer-wrap">
                                                {!atBottom && state.messages.length > 0 && (
                                                    <button
                                                        type="button"
                                                        className="chat-jump-bottom"
                                                        aria-label="Jump to latest message"
                                                        onClick={jumpToBottom}>
                                                        <IconArrowDown size={14} />
                                                    </button>
                                                )}
                                                {(subagents.length > 0 || state.tasks.length > 0) && (
                                                    <LiveStack subagents={subagents} tasks={state.tasks} folds={liveFolds} />
                                                )}
                                                <SharedChatFoot snapshot={snapshot} />
                                            </div>
                                        </div>
                                    </div>
                                </div>
                            </section>
                        </ReaderScrollContext.Provider>
                    </ChatSubagentsContext.Provider>
                </ChatAgentContext.Provider>
            </PathRootsProvider>
        </RemoteOriginContext>
    );
}

function SharedChatFoot({ snapshot }: { snapshot: ShareSnapshot }) {
    const [now, setNow] = useState(Date.now);
    useEffect(() => {
        const timer = window.setInterval(() => setNow(Date.now()), 60_000);
        return () => window.clearInterval(timer);
    }, []);
    const live = snapshot.status === "live" || snapshot.status === "missing";
    const state = !live
        ? null
        : snapshot.hostOnline
          ? snapshot.agent.running
              ? "Working now"
              : "Live"
          : "The sharer's computer is offline, so this is the chat as it last was";
    return (
        <div className="shared-chat-foot" role="status">
            <span className={`shared-chat-state${snapshot.hostOnline && live ? " live" : ""}`}>{state ?? "Read-only"}</span>
            {live && snapshot.expiresAt !== null && <span>Link ends in {endsIn(snapshot.expiresAt, now)}</span>}
        </div>
    );
}
