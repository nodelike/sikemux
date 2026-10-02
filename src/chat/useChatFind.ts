import { useEffect, useMemo, useRef, useState, type RefObject } from "react";
import type { Virtualizer } from "@tanstack/react-virtual";
import { findMatches, findPattern, rangesIn, type ChatFindOptions } from "./chatSearch";
import type { ChatMessage } from "./types";

const MATCHES = "chat-find";
const CURRENT = "chat-find-current";

const canHighlight = (): boolean => typeof CSS !== "undefined" && "highlights" in CSS && typeof Highlight !== "undefined";

/**
 * Find in a chat's transcript. The transcript only renders the rows near the
 * view, so matches are counted from the messages themselves, the transcript is
 * scrolled to the message holding the current one, and whatever is rendered is
 * then marked with the browser's highlights, which leave the rendered text as it is.
 */
export function useChatFind({
    visible,
    messages,
    scrollRef,
    virtualizer,
    onLeaveBottom,
}: {
    visible: boolean;
    messages: readonly ChatMessage[];
    scrollRef: RefObject<HTMLDivElement | null>;
    virtualizer: Virtualizer<HTMLDivElement, Element>;
    onLeaveBottom: () => void;
}) {
    const [query, setQuery] = useState("");
    const [options, setOptions] = useState<ChatFindOptions>({ caseSensitive: false, wholeWord: false });
    const [current, setCurrent] = useState(0);
    const inputRef = useRef<HTMLInputElement>(null);
    const reveal = useRef(false);

    const pattern = useMemo(() => findPattern(query, options), [query, options]);
    const matches = useMemo(() => findMatches(messages, pattern), [messages, pattern]);
    const index = matches.length > 0 ? Math.min(current, matches.length - 1) : 0;
    const target = matches[index];

    const targetMessage = target?.message;
    const targetOccurrence = target?.occurrence;
    useEffect(() => {
        if (targetMessage === undefined) return;
        onLeaveBottom();
        reveal.current = true;
        virtualizer.scrollToIndex(targetMessage, { align: "center" });
        // eslint-disable-next-line react-hooks/exhaustive-deps -- scroll only when the match moves
    }, [targetMessage, targetOccurrence]);

    useEffect(() => {
        const scroller = scrollRef.current;
        if (!visible || !pattern || !scroller || !canHighlight()) return;
        let frame = 0;
        const paint = () => {
            frame = 0;
            const all: Range[] = [];
            let focus: Range | undefined;
            for (const row of scroller.querySelectorAll<HTMLElement>(".chat-virtual-row")) {
                const ranges = rangesIn(row, pattern);
                all.push(...ranges);
                if (targetMessage === Number(row.dataset.index) && targetOccurrence !== undefined)
                    focus = ranges[Math.min(targetOccurrence, ranges.length - 1)];
            }
            CSS.highlights.set(MATCHES, new Highlight(...all));
            if (!focus) {
                CSS.highlights.delete(CURRENT);
                return;
            }
            CSS.highlights.set(CURRENT, new Highlight(focus));
            // A long message can hold the match well outside the view even once its row is centred.
            if (!reveal.current) return;
            reveal.current = false;
            const view = scroller.getBoundingClientRect();
            const box = focus.getBoundingClientRect();
            if (box.top < view.top || box.bottom > view.bottom) scroller.scrollTop += box.top - view.top - view.height / 2;
        };
        const schedule = () => {
            if (!frame) frame = requestAnimationFrame(paint);
        };
        schedule();
        scroller.addEventListener("scroll", schedule);
        const observer = new MutationObserver(schedule);
        observer.observe(scroller, { childList: true, subtree: true, characterData: true });
        return () => {
            cancelAnimationFrame(frame);
            scroller.removeEventListener("scroll", schedule);
            observer.disconnect();
            CSS.highlights.delete(MATCHES);
            CSS.highlights.delete(CURRENT);
        };
    }, [visible, pattern, targetMessage, targetOccurrence, scrollRef]);

    return {
        inputRef,
        query,
        setQuery: (next: string) => {
            setQuery(next);
            setCurrent(0);
        },
        options,
        setOptions: (next: ChatFindOptions) => {
            setOptions(next);
            setCurrent(0);
        },
        current: index,
        total: matches.length,
        move: (step: 1 | -1) => {
            if (matches.length === 0) return;
            setCurrent((index + step + matches.length) % matches.length);
            reveal.current = true;
        },
    };
}
