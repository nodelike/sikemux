import { useEffect, useMemo, useRef, useState, type RefObject } from "react";
import { findMatches, findPattern, rangesIn, type ChatFindOptions } from "./chatSearch";
import type { ChatMessage } from "./types";

const MATCHES = "chat-find";
const CURRENT = "chat-find-current";

const canHighlight = (): boolean => typeof CSS !== "undefined" && "highlights" in CSS && typeof Highlight !== "undefined";

/**
 * Find in a chat's transcript. Matches are counted from the messages
 * themselves, the transcript is scrolled to the message holding the current
 * one, and the rows are marked with the browser's highlights, which leave the
 * rendered text as it is. A long transcript mounts its oldest rows last, so a
 * match in one not mounted yet is shown once it is.
 */
export function useChatFind({
    visible,
    messages,
    scrollRef,
}: {
    visible: boolean;
    messages: readonly ChatMessage[];
    scrollRef: RefObject<HTMLDivElement | null>;
}) {
    const [query, setQuery] = useState("");
    const [options, setOptions] = useState<ChatFindOptions>({ caseSensitive: false, wholeWord: false });
    const [current, setCurrent] = useState(0);
    const inputRef = useRef<HTMLInputElement>(null);
    /* Bringing a match into view is two steps: its row first, which draws the
       row at its real height, then the match inside it on the next frame. */
    const reveal = useRef<"row" | "match" | null>(null);

    const pattern = useMemo(() => findPattern(query, options), [query, options]);
    const matches = useMemo(() => findMatches(messages, pattern), [messages, pattern]);
    const index = matches.length > 0 ? Math.min(current, matches.length - 1) : 0;
    const target = matches[index];

    const targetMessage = target?.message;
    const targetOccurrence = target?.occurrence;
    useEffect(() => {
        if (targetMessage !== undefined) reveal.current = "row";
    }, [targetMessage, targetOccurrence]);

    useEffect(() => {
        const scroller = scrollRef.current;
        if (!visible || !pattern || !scroller || !canHighlight()) return;
        let frame = 0;
        const paint = () => {
            frame = 0;
            const all: Range[] = [];
            let focus: Range | undefined;
            if (reveal.current === "row") {
                const row = scroller.querySelector(`.chat-row[data-index="${targetMessage}"]`);
                if (!row) return;
                row.scrollIntoView({ block: "center" });
                reveal.current = "match";
                frame = requestAnimationFrame(paint);
                return;
            }
            // Every row is mounted, but only the ones near the view are worth marking.
            const view = scroller.getBoundingClientRect();
            for (const row of scroller.querySelectorAll<HTMLElement>(".chat-row")) {
                const box = row.getBoundingClientRect();
                const target = targetMessage === Number(row.dataset.index);
                if (!target && (box.bottom < view.top - view.height || box.top > view.bottom + view.height)) continue;
                const ranges = rangesIn(row, pattern);
                all.push(...ranges);
                if (target && targetOccurrence !== undefined) focus = ranges[Math.min(targetOccurrence, ranges.length - 1)];
            }
            CSS.highlights.set(MATCHES, new Highlight(...all));
            if (!focus) {
                CSS.highlights.delete(CURRENT);
                return;
            }
            CSS.highlights.set(CURRENT, new Highlight(focus));
            // A long message can hold the match well outside the view even once its row is centred.
            if (reveal.current !== "match") return;
            reveal.current = null;
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
            reveal.current = "row";
        },
    };
}
