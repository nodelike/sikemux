import { useEffect, useRef } from "react";
import type { ISearchResultChangeEvent } from "@xterm/addon-search";
import type { TerminalController } from "./useXterm";
import type { TerminalSearchOptions } from "./interactions";
import { IconArrowDown, IconArrowUp, IconClose } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";

export function TerminalFindBar({
    controller,
    query,
    onQueryChange,
    options,
    onOptionsChange,
    result,
    onClose,
}: {
    controller: TerminalController;
    query: string;
    onQueryChange: (query: string) => void;
    options: TerminalSearchOptions;
    onOptionsChange: (options: TerminalSearchOptions) => void;
    result: ISearchResultChangeEvent;
    onClose: () => void;
}) {
    const inputRef = useRef<HTMLInputElement>(null);

    useEffect(() => {
        inputRef.current?.focus();
        inputRef.current?.select();
    }, []);

    useEffect(() => {
        controller.find(query, "next", options, true);
    }, [controller, query, options]);

    const move = (direction: "next" | "previous") => {
        if (query) controller.find(query, direction, options);
    };
    const toggle = (key: keyof TerminalSearchOptions) => onOptionsChange({ ...options, [key]: !options[key] });
    const resultLabel = result.resultCount > 0 && result.resultIndex >= 0 ? `${result.resultIndex + 1}/${result.resultCount}` : "0/0";

    return (
        <div className="terminal-find" role="search" onMouseDown={(event) => event.stopPropagation()}>
            <input
                ref={inputRef}
                value={query}
                onChange={(event) => onQueryChange(event.target.value)}
                onKeyDown={(event) => {
                    if (event.key === "Escape") {
                        event.preventDefault();
                        onClose();
                    } else if (event.key === "Enter") {
                        event.preventDefault();
                        move(event.shiftKey ? "previous" : "next");
                    }
                }}
                placeholder="Find in terminal"
                aria-label="Find in terminal"
                spellCheck={false}
            />
            <span className="terminal-find-result" aria-live="polite">
                {resultLabel}
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
            <Tooltip label="Use regular expression">
                <button
                    type="button"
                    className={options.regex ? "active" : ""}
                    aria-label="Use regular expression"
                    aria-pressed={options.regex}
                    onClick={() => toggle("regex")}>
                    .*
                </button>
            </Tooltip>
            <Tooltip label="Previous match (Shift+Enter)">
                <button type="button" onClick={() => move("previous")} aria-label="Previous match">
                    <IconArrowUp size={12} />
                </button>
            </Tooltip>
            <Tooltip label="Next match (Enter)">
                <button type="button" onClick={() => move("next")} aria-label="Next match">
                    <IconArrowDown size={12} />
                </button>
            </Tooltip>
            <Tooltip label="Close (Escape)">
                <button type="button" onClick={onClose} aria-label="Close terminal find">
                    <IconClose size={11} />
                </button>
            </Tooltip>
        </div>
    );
}
