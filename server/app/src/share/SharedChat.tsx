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
  IconExternal,
  IconEye,
} from "@shared/ui/Icons";
import {
  sharedChatTitle,
  type ShareSession,
  type ShareSnapshot,
} from "@shared/share/session";
import { appShareLink, type ShareLink } from "@shared/share/link";
import { GET_SIKEMUX, ReaderBar } from "./ReaderBar.tsx";
import {
  homeAction,
  ShareGate,
  type GateAction,
  type GateIcon,
  type GateTone,
} from "./ShareGate.tsx";
import { agentLabel } from "@shared/agents/catalog";
import "../chat/styles/chat.css";
import "../chat/styles/share.css";
import { formatTimeLeft } from "@shared/lib/format";
import { useNow } from "../chat/ui/useNow";

const SOON_MS = 60 * 60_000;

type GateCopy = {
  title: string;
  lede?: string;
  icon: GateIcon;
  tone: GateTone;
  action?: GateAction;
  foot?: boolean;
};

function gateFor(snapshot: ShareSnapshot): GateCopy | null {
  const away = homeAction(snapshot.signedIn);
  switch (snapshot.status) {
    case "connecting":
      return {
        title: "Opening the shared chat…",
        icon: "message",
        tone: "waiting",
        foot: false,
      };
    case "missing":
      return {
        title: "This chat isn’t running on its host right now",
        lede: "Ask whoever shared it to open the chat on their computer, or try again once it’s back.",
        icon: "laptop",
        tone: "amber",
        action: { label: "Try again", onClick: () => location.reload() },
      };
    case "notFound":
      return {
        title: "No chat is shared at this link",
        lede: "The link may be mistyped or removed. Check it, or ask whoever shared it for a new one.",
        icon: "message",
        tone: "failed",
        action: away,
      };
    case "unreadable":
      return {
        title: "This link doesn’t open the chat it points at",
        lede: "The key at the end of the link doesn’t match. Ask whoever shared it to copy the whole link again.",
        icon: "lock",
        tone: "failed",
        action: away,
      };
    case "ended":
      return snapshot.endedBecause === "expired"
        ? {
            title: "This share has expired",
            lede: "Share links end after the time set by the sharer. Ask whoever shared it for a new link.",
            icon: "clock",
            tone: "waiting",
            action: away,
          }
        : {
            title: "Sharing this chat has stopped",
            lede: "Whoever shared it turned off this link. Ask them for a new one if you still need it.",
            icon: "message",
            tone: "waiting",
            action: away,
          };
    default:
      return null;
  }
}

/**
 * Someone's chat, shared: a top bar, the transcript, a composer whose messages wait for the
 * sharer to send or dismiss, and a footer saying how long the link stays open. Someone reading
 * without signing in gets a reader bar, heading, and a quiet note on Sikemux above the transcript.
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
  const title = sharedChatTitle(snapshot);
  const guests = state.guests ?? [];
  const reader = !snapshot.signedIn;
  const startedAtTop = useRef(false);

  useEffect(() => {
    if (!reader || startedAtTop.current || state.messages.length === 0) return;
    const scroller = scrollRef.current;
    if (!scroller) return;
    startedAtTop.current = true;
    scroller.scrollTop = -scroller.scrollHeight;
  }, [reader, state.messages.length]);

  useEffect(() => {
    if (state.messages.length > 0 && title)
      document.title = `${title} · Sikemux`;
  }, [state.messages.length, title]);

  const gate = state.messages.length === 0 ? gateFor(snapshot) : null;
  if (gate) return <ShareGate {...gate} />;

  const messageCount =
    state.messages.length === 1
      ? "1 message"
      : `${state.messages.length} messages`;
  const metaParts = [
    chat?.model,
    state.messages.length > 0 ? messageCount : null,
  ].filter(Boolean);

  const before = (
    <>
      {reader && (
        <div className="sh-reader-intro">
          <div className="sh-reader-head">
            <h1>{title}</h1>
            <p className="sh-reader-meta">
              <span className={`agent-glyph ${type}`} aria-hidden="true">
                <AgentIcon type={type} size={14} />
              </span>
              <span>{agentLabel(type)}</span>
              {metaParts.map((part) => (
                <span key={part} className="sh-meta-item">
                  <i aria-hidden="true" />
                  {part}
                </span>
              ))}
            </p>
          </div>
          <aside className="sh-reader-callout">
            <span>
              <b>Watching someone’s agents work?</b>
              Sikemux runs yours side by side, on your own computers.
            </span>
            <a className="sh-btn q" href={GET_SIKEMUX}>
              See how it works
            </a>
          </aside>
        </div>
      )}
      {snapshot.hasEarlier && (
        <EarlierTurns
          state={snapshot.earlier}
          onLoad={session.loadEarlier}
          scrollRef={scrollRef}
        />
      )}
    </>
  );

  const canWrite = snapshot.status === "live" && snapshot.hostOnline;

  return (
    <RemoteOriginContext value={origin}>
      <PathRootsProvider cwd={chat?.cwd ?? ""} home={chat?.home ?? undefined}>
        <ChatAgentContext.Provider value={type}>
          <ChatSubagentsContext.Provider value={subagents}>
            <ToolDetailContext.Provider value={toolDetail}>
              <section
                className={`agent-surface shared-chat${reader ? " shared-reader" : ""}`}
              >
                {reader ? (
                  <ReaderBar
                    badge={
                      <span className="sh-tag">
                        <IconEye size={13} aria-hidden="true" />
                        <span>Read-only</span>
                      </span>
                    }
                  />
                ) : (
                  <header className="sh-bar sh-chat-bar">
                    <span
                      className={`sh-agent-mark agent-glyph ${type}`}
                      aria-hidden="true"
                    >
                      <AgentIcon type={type} size={15} />
                    </span>
                    <h1 className="sh-chat-title" title={title}>
                      {title}
                    </h1>
                    <span className="sh-tag sh-guest-tag">
                      <IconEye size={13} aria-hidden="true" />
                      <span>Guest</span>
                    </span>
                    <span className="sh-gap" />
                    <button
                      type="button"
                      className="sh-btn q"
                      onClick={() => location.assign(appShareLink(link))}
                    >
                      <IconExternal size={14} aria-hidden="true" />
                      <span>Open in Sikemux</span>
                    </button>
                  </header>
                )}
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
                      />
                      <div className="chat-composer-wrap sh-composer-wrap">
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
                        {!reader && (
                          <GuestComposer
                            session={session}
                            canWrite={canWrite}
                            status={snapshot.status}
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

const NAME_KEY = "sikemux.share.guestName";

function rememberedName(): string {
  try {
    return localStorage.getItem(NAME_KEY) ?? "";
  } catch {
    return "";
  }
}

function rememberName(name: string) {
  try {
    localStorage.setItem(NAME_KEY, name.trim());
  } catch {
    return;
  }
}

/** Where a viewer writes to the agent; what it sends waits for the sharer to send or dismiss. */
function GuestComposer({
  session,
  canWrite,
  status,
  hostOnline,
}: {
  session: ShareSession;
  canWrite: boolean;
  status: ShareSnapshot["status"];
  hostOnline: boolean;
}) {
  const [text, setText] = useState("");
  const [name, setName] = useState(rememberedName);
  const [sending, setSending] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const ready = canWrite && !sending && text.trim() !== "";
  const placeholder = canWrite
    ? "Write to the agent. The sharer sends or dismisses it"
    : !hostOnline && status === "live"
      ? "The sharer’s computer is offline"
      : "You can write again when the chat is live";
  const send = () => {
    if (!ready) return;
    setSending(true);
    setProblem(null);
    rememberName(name);
    void session.sendMessage(text, name).then((refused) => {
      setSending(false);
      if (refused) setProblem(refused);
      else setText("");
    });
  };
  return (
    <form
      className={`shared-chat-composer${canWrite ? "" : " off"}`}
      onSubmit={(event) => {
        event.preventDefault();
        send();
      }}
    >
      <textarea
        value={text}
        rows={2}
        maxLength={10_000}
        placeholder={placeholder}
        disabled={!canWrite}
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
        <input
          value={name}
          maxLength={40}
          placeholder="Your name (optional)"
          aria-label="Your name, shown to the sharer"
          autoComplete="name"
          disabled={!canWrite}
          onChange={(event) => setName(event.target.value)}
        />
        <span role="status">{problem}</span>
        <button type="submit" disabled={!ready} aria-label="Send to the sharer">
          <IconArrowUp size={15} />
        </button>
      </div>
    </form>
  );
}

function SharedChatFoot({ snapshot }: { snapshot: ShareSnapshot }) {
  const now = useNow(true, 60_000);
  const live = snapshot.status === "live";
  const online = live && snapshot.hostOnline;
  const away = (live && !snapshot.hostOnline) || snapshot.status === "missing";
  const label = online
    ? snapshot.agent.running
      ? "Working now"
      : "Live"
    : snapshot.status === "ended"
      ? snapshot.endedBecause === "expired"
        ? "This share has expired. This is the chat as it last was."
        : "Not live. This is the chat as it last was."
      : snapshot.status === "missing"
        ? "This chat isn’t running on its host right now. This is the chat as it last was."
        : "The sharer’s computer is offline. This is the chat as it last was.";
  const soon =
    snapshot.expiresAt !== null && snapshot.expiresAt - now < SOON_MS;
  return (
    <div className="shared-chat-foot" role="status">
      <span
        className={`shared-chat-state${online ? " live" : away ? " away" : ""}`}
      >
        {label}
      </span>
      {(live || snapshot.status === "missing") &&
        snapshot.expiresAt !== null && (
          <span className={soon ? "sh-soon" : undefined}>
            Link ends in {formatTimeLeft(snapshot.expiresAt, now)}
          </span>
        )}
    </div>
  );
}
