import { useEffect, useRef, type RefObject } from "react";

/** Loads the turns before the first one shown as the reader scrolls up to it, a page at a time. */
export function EarlierTurns({
  state,
  onLoad,
  scrollRef,
}: {
  state: "idle" | "loading" | "failed";
  onLoad: () => void;
  scrollRef: RefObject<HTMLDivElement | null>;
}) {
  const edge = useRef<HTMLDivElement>(null);
  const idle = state === "idle";
  useEffect(() => {
    const root = scrollRef.current;
    const target = edge.current;
    if (!idle || !root || !target) return;
    const observer = new IntersectionObserver(
      (entries) => entries.some((entry) => entry.isIntersecting) && onLoad(),
      {
        root,
        rootMargin: "600px 0px 0px 0px",
      },
    );
    observer.observe(target);
    return () => observer.disconnect();
  }, [idle, onLoad, scrollRef]);
  return (
    <div ref={edge} className="chat-earlier" role="status">
      {state === "loading" && (
        <>
          <span className="chat-activity-loader" aria-hidden="true" />
          <span>Loading earlier messages…</span>
        </>
      )}
      {state === "failed" && (
        <>
          <span>Couldn't load earlier messages.</span>
          <button type="button" onClick={onLoad}>
            Try again
          </button>
        </>
      )}
    </div>
  );
}
