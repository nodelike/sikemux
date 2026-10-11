import { useLayoutEffect, useRef, type RefObject } from "react";
import { animate } from "../lib/motion";
import type { ChatMessage } from "./types";

/* A message that has just arrived rises into place. Only new ones: a
   transcript restored or mounted all at once just shows. */
export function useMessageArrival(scrollRef: RefObject<HTMLDivElement | null>, messages: ChatMessage[]): void {
    const shownMessages = useRef<Set<string> | null>(null);
    useLayoutEffect(() => {
        const ids = messages.map((message) => message.id);
        const shown = shownMessages.current;
        if (!shown) {
            shownMessages.current = new Set(ids);
            return;
        }
        const fresh = ids.filter((id) => !shown.has(id));
        for (const id of fresh) shown.add(id);
        if (fresh.length === 0 || fresh.length > 2) return;
        for (const id of fresh) {
            const row = scrollRef.current?.querySelector<HTMLElement>(`.chat-row[data-index="${ids.indexOf(id)}"] > *`);
            animate(
                row,
                [
                    { opacity: 0, transform: "translateY(10px) scale(0.985)" },
                    { opacity: 1, transform: "none" },
                ],
                { duration: 200 },
            );
        }
    }, [messages, scrollRef]);
}
