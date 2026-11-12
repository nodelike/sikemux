import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import {
  ChatAgentContext,
  ChatSubagentsContext,
  ToolDetailContext,
} from "../chat/transcript/chatAgent";
import { warmTranscript } from "../chat/transcript/ChatMessageRow";
import { ChatTranscript } from "../chat/transcript/ChatTranscript";
import { EarlierTurns } from "../chat/transcript/EarlierTurns";
import { activityText } from "@shared/chat/chatStatus";
import { PathRootsProvider } from "../chat/files/FileRef";
import { ImageViewer } from "../chat/files/ImageViewer";
import LiveStack from "../chat/transcript/LiveStack";
import { newFoldMemory } from "@shared/chat/longText";
import { RemoteOriginContext } from "../chat/transcript/origin";
import { subagentsOf } from "@shared/chat/subagents";
import { useMessageArrival } from "../chat/transcript/useMessageArrival";
import { useStickToBottom } from "../chat/transcript/useStickToBottom";
import {
  AgentIcon,
  IconArrowDown,
  IconArrowUp,
  IconEye,
  IconPlug,
} from "@shared/ui/Icons";
import {
  sharedChatTitle,
  type ShareSession,
  type ShareSnapshot,
} from "@shared/share/session";
import { appShareLink, type ShareLink } from "@shared/share/link";
import "../chat/styles/chat.css";
import "../chat/styles/share.css";
import { formatTimeLeft } from "@shared/lib/format";
import { useNow } from "../chat/ui/useNow";

const PROBLEMS: Partial<Record<ShareSnapshot["status"], string>> = {
  missing: "This chat is not running on its host right now.",
  notFound: "No chat is shared at this link.",
  signedOut: "Sign in to Sikemux to watch shared chats.",
  unreadable:
    "This link does not open the chat it points at. Check that the whole link was copied.",
};

function endedText(snapshot: ShareSnapshot): string {
  return snapshot.endedBecause === "expired"
    ? "This share has expired."
    : "Sharing this chat has stopped.";
}

/**
 * Someone's chat, shared: the same header and transcript as the agent's own pane, a composer
 * whose messages wait for the sharer to send or dismiss, and a line saying how long it stays open.
 */
export function SharedChat({
  session,
  link,
}: {
  session: ShareSession;
  link: ShareLink;
}) {
  const snapshot = useSyncExternalStore(session.subscribe, session.snapshot);
  const { agent: state, chat } = snapshot;
  const [foldMemory] = useState(newFoldMemory);
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  useMessageArrival(scrollRef, state.messages);
  useEffect(() => void warmTranscript(state.messages), [state.messages]);
  const { atBottom, onScroll, jumpToBottom } = useStickToBottom({
    scrollRef,
    contentRef,
  });
  const [foldedLive, setFoldedLive] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
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

  const type = chat?.provider ?? "agent";
  const origin = useMemo(() => ({ readImage: session.readImage }), [session]);
  const toolDetail = useCallback(
    (toolCallId: string) =>
      session.readToolDetail(toolCallId).then((detail) => detail ?? {}),
    [session],
  );
  const activity =
    snapshot.status === "live" ? activityText(state.activity) : null;
  const subagents = useMemo(
    () => subagentsOf(state.messages, state.subagentRows),
    [state.messages, state.subagentRows],
  );
  const problem =
    snapshot.status === "ended"
      ? endedText(snapshot)
      : PROBLEMS[snapshot.status];
  const title = sharedChatTitle(snapshot);
  const guests = state.guests ?? [];

  const before = (
    <>
      {snapshot.hasEarlier && (
        <EarlierTurns
          state={snapshot.earlier}
          onLoad={session.loadEarlier}
          scrollRef={scrollRef}
        />
      )}
      {state.messages.length === 0 && (
        <div
          className={`chat-connection-state ${problem ? "stopped" : "connecting"}`}
          role="status"
        >
          {!problem && (
            <span className="chat-activity-loader" aria-hidden="true" />
          )}
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
        <ChatAgentContext.Provider value={type}>
          <ChatSubagentsContext.Provider value={subagents}>
            <ToolDetailContext.Provider value={toolDetail}>
              <section className="agent-surface shared-chat">
                <header className="agent-surface-header">
                  <div className="agent-surface-name">
                    <span
                      className={`agent-surface-mark agent-glyph ${type}`}
                      aria-hidden="true"
                    >
                      <AgentIcon type={type} size={16} />
                    </span>
                    <span className="agent-surface-title" title={title}>
                      {title}
                    </span>
                  </div>
                  <div className="agent-surface-controls">
                    <span className="shared-chat-badge">
                      <IconEye size={13} />
                      <span>Guest</span>
                    </span>
                    <div className="agent-view-switch">
                      <button
                        type="button"
                        onClick={() => location.assign(appShareLink(link))}
                      >
                        Open in Sikemux
                      </button>
                    </div>
                  </div>
                </header>
                <div className="agent-surface-body">
                  <div className="agent-gui-layer">
                    <div className="agent-chat-pane">
                      <ChatTranscript
                        state={state}
                        foldMemory={foldMemory}
                        scrollRef={scrollRef}
                        contentRef={contentRef}
                        onScroll={onScroll}
                        activity={activity}
                        before={before}
                        after={after}
                      />
                      <div className="chat-composer-wrap">
                        {!atBottom && state.messages.length > 0 && (
                          <button
                            type="button"
                            className="chat-jump-bottom"
                            aria-label="Jump to latest message"
                            onClick={jumpToBottom}
                          >
                            <IconArrowDown size={14} />
                          </button>
                        )}
                        {(guests.length > 0 ||
                          state.runningSubagents.length > 0 ||
                          state.tasks.length > 0) && (
                          <LiveStack
                            guests={guests}
                            subagents={state.runningSubagents}
                            tasks={state.tasks}
                            folds={liveFolds}
                          />
                        )}
                        {snapshot.status === "live" && (
                          <GuestComposer
                            session={session}
                            hostOnline={snapshot.hostOnline}
                          />
                        )}
                        <SharedChatFoot snapshot={snapshot} />
                      </div>
                    </div>
                  </div>
                </div>
                <ImageViewer />
              </section>
            </ToolDetailContext.Provider>
          </ChatSubagentsContext.Provider>
        </ChatAgentContext.Provider>
      </PathRootsProvider>
    </RemoteOriginContext>
  );
}

/** Where a viewer writes to the agent; what it sends waits for the sharer to send or dismiss. */
function GuestComposer({
  session,
  hostOnline,
}: {
  session: ShareSession;
  hostOnline: boolean;
}) {
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const ready = hostOnline && !sending && text.trim() !== "";
  const send = () => {
    if (!ready) return;
    setSending(true);
    setProblem(null);
    void session.sendMessage(text).then((refused) => {
      setSending(false);
      if (refused) setProblem(refused);
      else setText("");
    });
  };
  return (
    <form
      className="shared-chat-composer"
      onSubmit={(event) => {
        event.preventDefault();
        send();
      }}
    >
      <textarea
        value={text}
        rows={2}
        maxLength={10_000}
        placeholder={
          hostOnline
            ? "Write to the agent. The sharer sends or dismisses it"
            : "The sharer's computer is offline"
        }
        disabled={!hostOnline}
        aria-label="Message for the agent"
        onChange={(event) => setText(event.target.value)}
        onKeyDown={(event) => {
          if (
            event.key === "Enter" &&
            !event.shiftKey &&
            !event.nativeEvent.isComposing
          ) {
            event.preventDefault();
            send();
          }
        }}
      />
      <div className="shared-chat-composer-foot">
        <span role="status">{problem}</span>
        <button type="submit" disabled={!ready} aria-label="Send to the sharer">
          <IconArrowUp size={14} />
        </button>
      </div>
    </form>
  );
}

function SharedChatFoot({ snapshot }: { snapshot: ShareSnapshot }) {
  const now = useNow(true, 60_000);
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
      <span
        className={`shared-chat-state${snapshot.hostOnline && live ? " live" : ""}`}
      >
        {state ?? "Not live"}
      </span>
      {live && snapshot.expiresAt !== null && (
        <span>Link ends in {formatTimeLeft(snapshot.expiresAt, now)}</span>
      )}
    </div>
  );
}
