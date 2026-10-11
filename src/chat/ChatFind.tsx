import { useEffect, type RefObject } from "react";
import { ChatFindBar } from "./ChatFindBar";
import { useChatFind } from "./useChatFind";
import type { ChatMessage } from "./types";
import "../styles/chatFind.css";

/** Find in a chat, loaded the first time it is asked for. `request` changes each time it is, and puts the caret back in the query. */
export default function ChatFind({
    request,
    visible,
    messages,
    scrollRef,
    onClose,
}: {
    request: number;
    visible: boolean;
    messages: readonly ChatMessage[];
    scrollRef: RefObject<HTMLDivElement | null>;
    onClose: () => void;
}) {
    const find = useChatFind({ visible, messages, scrollRef });
    const { inputRef } = find;

    useEffect(() => {
        inputRef.current?.focus();
        inputRef.current?.select();
    }, [request, inputRef]);

    return (
        <ChatFindBar
            inputRef={inputRef}
            query={find.query}
            onQueryChange={find.setQuery}
            options={find.options}
            onOptionsChange={find.setOptions}
            current={find.current}
            total={find.total}
            onMove={find.move}
            onClose={onClose}
        />
    );
}
