import { useCallback, useLayoutEffect, useRef, useState, type RefObject, type UIEvent } from "react";

// How far above the last line still counts as reading the latest message.
export const BOTTOM_SLACK = 72;

export function useStickToBottom({
    scrollRef,
    contentRef,
    visible,
    messageCount,
    revision,
}: {
    scrollRef: RefObject<HTMLDivElement | null>;
    contentRef: RefObject<HTMLDivElement | null>;
    visible: boolean;
    messageCount: number;
    revision: number;
}) {
    const [atBottom, setAtBottom] = useState(true);
    const stickToBottomRef = useRef(true);
    const lastScrollTopRef = useRef(0);
    const lastGestureRef = useRef(0);

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
    }, [scrollRef]);

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
        const content = contentRef.current;
        if (!content || typeof ResizeObserver === "undefined") return;
        const observer = new ResizeObserver(() => {
            if (stickToBottomRef.current) pinToBottom();
        });
        observer.observe(content);
        return () => observer.disconnect();
    }, [contentRef, pinToBottom]);

    useLayoutEffect(() => {
        if (!visible || !stickToBottomRef.current || messageCount === 0) return;
        pinToBottom();
    }, [messageCount, revision, pinToBottom, visible]);

    const onScroll = (event: UIEvent<HTMLDivElement>) => {
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
    };

    const jumpToBottom = () => {
        stickToBottomRef.current = true;
        setAtBottom(true);
        pinToBottom();
    };

    /** Stops holding the bottom, for a scroll this pane makes itself up into the transcript. */
    const leaveBottom = () => {
        stickToBottomRef.current = false;
        setAtBottom(false);
    };

    return { atBottom, noteGesture, onScroll, jumpToBottom, leaveBottom };
}
