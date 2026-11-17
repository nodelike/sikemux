import {
  useEffect,
  useMemo,
  useState,
  type ReactNode,
  type RefObject,
  type UIEvent,
} from "react";
import { ChatActivity } from "./ChatActivity";
import { ChatMessageRow } from "./ChatMessageRow";
import { FoldMemoryContext, type FoldMemory } from "@shared/chat/longText";
import { copyText, EARLIER_LOST, rowTime } from "@shared/chat/messageMeta";
import { PermissionRequest } from "./PermissionRequest";
import type { ChatMessage } from "@shared/wire.generated";
import type { ChatRows } from "@shared/chat/rows";
import { WorkSummary } from "./WorkSummary";
import { TeamFold } from "./TeamFold";
import { teamRows, type RunOpen } from "@shared/chat/team";

/* A long transcript mounts its newest rows first and the older ones above
   them a slice at a time. Laid out from the bottom, rows added above the view
   never move it. */
const FIRST_ROWS = 40;

/** Where each finished turn that folds starts and ends among the rows, by the index of each row in it. */
function foldSpans(
  messages: readonly ChatMessage[],
): Map<number, { start: number; end: number }> {
  const spans = new Map<number, { start: number; end: number }>();
  let start = -1;
  for (let index = 0; index < messages.length; index += 1) {
    const fold = messages[index].fold;
    if (!fold) continue;
    if (fold.id === messages[index].id || start < 0) start = index;
    if (fold.last !== messages[index].id) continue;
    for (let at = start; at <= index; at += 1)
      spans.set(at, { start, end: index });
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
    const timer = window.setTimeout(
      () => setStart((current) => Math.max(0, current - ROWS_PER_SLICE)),
      16,
    );
    return () => window.clearTimeout(timer);
  }, [start]);
  return Math.min(start, count);
}

/**
 * The scrolling part of a chat: its messages, folded into work summaries,
 * what the agent is doing now, its plan and what it waits on.
 */
export function ChatTranscript({
  state,
  foldMemory,
  scrollRef,
  contentRef,
  onScroll,
  activity,
  before,
  after,
}: {
  state: ChatRows;
  foldMemory: FoldMemory;
  scrollRef: RefObject<HTMLDivElement | null>;
  contentRef: RefObject<HTMLDivElement | null>;
  onScroll: (event: UIEvent<HTMLDivElement>) => void;
  activity: string | null;
  /** Drawn above the messages. */
  before?: ReactNode;
  /** Drawn below everything else. */
  after?: ReactNode;
}) {
  const firstRow = useOlderRows(state.messages.length);
  const spans = useMemo(() => foldSpans(state.messages), [state.messages]);
  const [openWork, setOpenWork] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const toggleWork = (id: string) =>
    setOpenWork((open) => {
      const next = new Set(open);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  const runs = useMemo(
    () =>
      teamRows(state.messages, (id): RunOpen =>
        openWork.has(`team-all/${id}`)
          ? "all"
          : openWork.has(`team/${id}`)
            ? "open"
            : "closed",
      ),
    [state.messages, openWork],
  );

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
            const run = runs.get(message.id);
            const head = run?.head && (
              <TeamFold
                line={run.head}
                open={openWork.has(`team/${run.id}`)}
                all={openWork.has(`team-all/${run.id}`)}
                onToggle={() => toggleWork(`team/${run.id}`)}
                onAll={() => toggleWork(`team-all/${run.id}`)}
              />
            );
            if (run?.hidden)
              return (
                <div key={message.id} data-index={index} className="chat-row">
                  {head}
                </div>
              );
            if (run)
              return (
                <div key={message.id} data-index={index} className="chat-row">
                  {head}
                  <ChatMessageRow
                    kept={run.shown}
                    message={message}
                    live={state.running && index === state.messages.length - 1}
                    copyable={copyText(state.messages, index)}
                    rate={message.meta?.rate ?? null}
                    at={rowTime(message)}
                    took={message.meta?.took ?? null}
                  />
                </div>
              );
            const meta = message.meta;
            const span = spans.get(index);
            const fold = span && message.fold;
            const workId = fold ? fold.id : "";
            const folded = span !== undefined && !openWork.has(workId);
            const summary = span &&
              fold &&
              index === Math.max(span.start, firstRow) && (
                <WorkSummary
                  took={fold.took ?? null}
                  calls={fold.calls}
                  thought={fold.thought}
                  open={!folded}
                  onToggle={() => toggleWork(workId)}
                />
              );
            // A host older than v0.6.1-nightly.5 sends no `shown`, so its rows draw whole.
            if (span && fold && folded && fold.shown?.length === 0)
              return (
                <div key={message.id} data-index={index} className="chat-row">
                  {summary}
                </div>
              );
            return (
              <div key={message.id} data-index={index} className="chat-row">
                {summary}
                <ChatMessageRow
                  kept={span && fold && folded ? (fold.shown ?? null) : null}
                  message={message}
                  live={state.running && index === state.messages.length - 1}
                  copyable={copyText(state.messages, index)}
                  rate={meta?.rate ?? null}
                  at={rowTime(message)}
                  took={meta?.took ?? null}
                />
              </div>
            );
          })}
        </FoldMemoryContext>
        {activity &&
          (state.running ? (
            <ChatActivity label={activity} />
          ) : (
            <div className="chat-activity" role="status">
              <span className="chat-activity-loader" aria-hidden="true" />
              <span className="chat-activity-label">{activity}</span>
            </div>
          ))}
        {state.plan && state.plan.length > 0 && (
          <details className="chat-plan">
            <summary>Plan</summary>
            <ul className="chat-plan-steps">
              {state.plan.map((entry, index) => (
                <li key={index} data-status={entry.status}>
                  {entry.content}
                </li>
              ))}
            </ul>
          </details>
        )}
        {state.permissions.map((request) => (
          <PermissionRequest key={request.requestId} request={request} />
        ))}
        {after}
      </div>
    </div>
  );
}
