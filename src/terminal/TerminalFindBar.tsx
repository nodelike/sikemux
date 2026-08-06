import { useEffect, useRef } from "react";
import type { ISearchResultChangeEvent } from "@xterm/addon-search";
import type { TerminalController } from "./useXterm";
import type { TerminalSearchOptions } from "./interactions";

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
            <button type="button" className={options.caseSensitive ? "active" : ""} onClick={() => toggle("caseSensitive")} title="Match case">
                Aa
            </button>
            <button type="button" className={options.wholeWord ? "active" : ""} onClick={() => toggle("wholeWord")} title="Match whole word">
                W
            </button>
            <button type="button" className={options.regex ? "active" : ""} onClick={() => toggle("regex")} title="Use regular expression">
                .*
            </button>
            <button type="button" onClick={() => move("previous")} title="Previous match (Shift+Enter)" aria-label="Previous match">
                ↑
            </button>
            <button type="button" onClick={() => move("next")} title="Next match (Enter)" aria-label="Next match">
                ↓
            </button>
            <button type="button" onClick={onClose} title="Close (Escape)" aria-label="Close terminal find">
                ×
            </button>
        </div>
    );
}
