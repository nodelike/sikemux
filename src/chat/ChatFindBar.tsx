import type { RefObject } from "react";
import { IconArrowDown, IconArrowUp, IconClose } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";
import type { ChatFindOptions } from "./chatSearch";

export function ChatFindBar({
    inputRef,
    query,
    onQueryChange,
    options,
    onOptionsChange,
    current,
    total,
    onMove,
    onClose,
}: {
    inputRef: RefObject<HTMLInputElement | null>;
    query: string;
    onQueryChange: (query: string) => void;
    options: ChatFindOptions;
    onOptionsChange: (options: ChatFindOptions) => void;
    current: number;
    total: number;
    onMove: (step: 1 | -1) => void;
    onClose: () => void;
}) {
    const toggle = (key: keyof ChatFindOptions) => onOptionsChange({ ...options, [key]: !options[key] });

    return (
        <div className="chat-find" role="search" onMouseDown={(event) => event.stopPropagation()}>
            <input
                ref={inputRef}
                value={query}
                onChange={(event) => onQueryChange(event.target.value)}
                onKeyDown={(event) => {
                    if (event.key === "Escape") {
                        event.preventDefault();
                        event.stopPropagation();
                        onClose();
                    } else if (event.key === "Enter") {
                        event.preventDefault();
                        onMove(event.shiftKey ? -1 : 1);
                    }
                }}
                placeholder="Find in chat"
                aria-label="Find in chat"
                spellCheck={false}
            />
            <span className="chat-find-result" aria-live="polite">
                {total > 0 ? `${current + 1}/${total}` : query ? "No results" : "0/0"}
            </span>
            <Tooltip label="Match case">
                <button
                    type="button"
                    className={options.caseSensitive ? "active" : ""}
                    aria-label="Match case"
                    aria-pressed={options.caseSensitive}
                    onClick={() => toggle("caseSensitive")}>
                    Aa
                </button>
            </Tooltip>
            <Tooltip label="Match whole word">
                <button
                    type="button"
                    className={options.wholeWord ? "active" : ""}
                    aria-label="Match whole word"
                    aria-pressed={options.wholeWord}
                    onClick={() => toggle("wholeWord")}>
                    W
                </button>
            </Tooltip>
            <Tooltip label="Previous match (Shift+Enter)">
                <button type="button" onClick={() => onMove(-1)} aria-label="Previous match" disabled={total === 0}>
                    <IconArrowUp size={12} />
                </button>
            </Tooltip>
            <Tooltip label="Next match (Enter)">
                <button type="button" onClick={() => onMove(1)} aria-label="Next match" disabled={total === 0}>
                    <IconArrowDown size={12} />
                </button>
            </Tooltip>
            <Tooltip label="Close (Escape)">
                <button type="button" onClick={onClose} aria-label="Close find in chat">
                    <IconClose size={11} />
                </button>
            </Tooltip>
        </div>
    );
}
