import { useEffect, useMemo, useRef, useState } from "react";
import { basename } from "../../../plugin-api/host";
import { IconBruno, IconClose, IconFolder, IconSearch, rankBy, useMouseActive } from "../../../plugin-api/ui";
import { brunoSettings, closePalettes, openBrunoFolder, openBrunoSession, removeBrunoWorkspace } from "../state";
import "../bruno.css";

/** Switches the loaded workspace, adds one, or forgets one. */
export function BrunoWorkspacePalette() {
    const workspaces = brunoSettings.useSelect((settings) => settings.workspaces);
    const loaded = brunoSettings.useSelect((settings) => settings.collectionPath);
    const [query, setQuery] = useState("");
    const [sel, setSel] = useState(0);
    const inputRef = useRef<HTMLInputElement>(null);
    const mouseActive = useMouseActive();

    const items = useMemo(
        () =>
            rankBy(
                query,
                workspaces.filter((path) => path !== loaded),
                (path) => [basename(path), path],
            ),
        [workspaces, loaded, query],
    );

    useEffect(() => inputRef.current?.focus(), []);

    const choose = (path: string | undefined) => {
        if (!path) return;
        closePalettes();
        openBrunoSession(path);
    };
    const addFolder = () => {
        closePalettes();
        void openBrunoFolder();
    };

    const onKeyDown = (event: React.KeyboardEvent) => {
        if (event.key === "Escape") closePalettes();
        else if (event.key === "ArrowDown" || (event.key === "Tab" && !event.shiftKey)) {
            event.preventDefault();
            setSel((index) => (items.length ? (index + 1) % items.length : 0));
        } else if (event.key === "ArrowUp" || (event.key === "Tab" && event.shiftKey)) {
            event.preventDefault();
            setSel((index) => (items.length ? (index - 1 + items.length) % items.length : 0));
        } else if (event.key === "Enter") {
            event.preventDefault();
            choose(items[sel]);
        }
    };

    return (
        <div className="picker-backdrop" onMouseDown={closePalettes}>
            <div
                className="picker"
                role="dialog"
                aria-modal="true"
                aria-label="Switch Bruno workspace"
                onMouseDown={(event) => event.stopPropagation()}>
                <div className="picker-input-wrap">
                    <IconSearch size={15} className="picker-search-icon" />
                    <input
                        ref={inputRef}
                        className="picker-input"
                        placeholder="switch bruno workspace…"
                        value={query}
                        onChange={(event) => {
                            setQuery(event.target.value);
                            setSel(0);
                        }}
                        onKeyDown={onKeyDown}
                        spellCheck={false}
                    />
                    <button className="picker-folder-btn" onClick={addFolder} title="Add a Bruno workspace folder" type="button">
                        <IconFolder size={14} />
                    </button>
                </div>
                <div className="picker-list">
                    {items.length === 0 && (
                        <div className="picker-empty">
                            {query.trim() ? (
                                "no matches"
                            ) : (
                                <button className="picker-link" onClick={addFolder}>
                                    add a Bruno workspace folder
                                </button>
                            )}
                        </div>
                    )}
                    {items.map((path, index) => (
                        <div
                            key={path}
                            className="picker-item-wrap"
                            onMouseEnter={() => {
                                if (mouseActive.current) setSel(index);
                            }}>
                            <button className={`picker-item${index === sel ? " sel" : ""}`} onClick={() => choose(path)}>
                                <span className="picker-icon bruno">
                                    <IconBruno size={14} />
                                </span>
                                <span className="picker-name">{basename(path)}</span>
                                <span className="picker-sub">{path}</span>
                            </button>
                            <button
                                type="button"
                                className="picker-forget"
                                aria-label={`Forget ${basename(path)} workspace`}
                                title="Forget this workspace"
                                onClick={(event) => {
                                    event.stopPropagation();
                                    removeBrunoWorkspace(path);
                                }}>
                                <IconClose size={11} />
                            </button>
                        </div>
                    ))}
                </div>
            </div>
        </div>
    );
}
