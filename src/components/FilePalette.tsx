import { useModalFocus } from "../hooks/useModalFocus";
import { useEffect, useMemo, useRef, useState } from "react";
import * as cmd from "../state/commands";
import { rankBy } from "../lib/fuzzy";
import { basename, dirname, joinPath } from "../lib/paths";
import { useResourceEnabled } from "../state/resources";
import { filesListR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { useMouseActive } from "../hooks/useMouseActive";
import { IconSearch } from "./Icons";
import { FileIcon } from "./FileIcon";

const MAX_RESULTS = 200;

export function FilePalette() {
    const modalRef = useRef<HTMLDivElement>(null);
    useModalFocus(modalRef);
    const session = useStore((s) => s.sessions[s.activeSessionId]);
    const cwd = session?.cwd ?? "";

    const [query, setQuery] = useState("");
    const [sel, setSel] = useState(0);
    const inputRef = useRef<HTMLInputElement>(null);
    const listRef = useRef<HTMLDivElement>(null);
    const mouseActive = useMouseActive();

    const list = useResourceEnabled(!!cwd, filesListR, cwd || "");
    const all = useMemo(() => (cwd ? (list.data ?? []) : []), [cwd, list.data]);

    useEffect(() => {
        inputRef.current?.focus();
    }, []);

    const items = useMemo(() => rankBy(query, all, (path) => [basename(path), path]).slice(0, MAX_RESULTS), [all, query]);

    useEffect(() => {
        setSel(0);
    }, [query, items.length]);

    useEffect(() => {
        const el = listRef.current?.querySelector<HTMLElement>(`.picker-item:nth-child(${sel + 1})`);
        el?.scrollIntoView({ block: "nearest" });
    }, [sel]);

    const activate = (path: string | undefined) => {
        if (!path || !cwd) return;
        cmd.requestOpenFile(joinPath(cwd, path));
        cmd.closeFilePalette();
    };

    const onKeyDown = (e: React.KeyboardEvent) => {
        if (e.key === "Escape") {
            cmd.closeFilePalette();
        } else if (e.key === "ArrowDown" || (e.key === "Tab" && !e.shiftKey)) {
            e.preventDefault();
            setSel((s) => (items.length ? (s + 1) % items.length : 0));
        } else if (e.key === "ArrowUp" || (e.key === "Tab" && e.shiftKey)) {
            e.preventDefault();
            setSel((s) => (items.length ? (s - 1 + items.length) % items.length : 0));
        } else if (e.key === "Enter") {
            e.preventDefault();
            activate(items[sel]);
        }
    };

    return (
        <div className="picker-backdrop" onMouseDown={cmd.closeFilePalette}>
            <div
                ref={modalRef}
                tabIndex={-1}
                className="picker"
                role="dialog"
                aria-modal="true"
                aria-label="Open file"
                onMouseDown={(e) => e.stopPropagation()}>
                <div className="picker-input-wrap">
                    <IconSearch size={15} className="picker-search-icon" />
                    <input
                        ref={inputRef}
                        className="picker-input"
                        aria-label="Search project files"
                        placeholder={cwd ? "search files…" : "no project — open one first"}
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        onKeyDown={onKeyDown}
                        spellCheck={false}
                        disabled={!cwd}
                    />
                </div>

                <div className="picker-list" ref={listRef}>
                    {items.length === 0 && (
                        <div className="picker-empty">{!cwd ? "open a project to search files" : all.length === 0 ? "indexing…" : "no matches"}</div>
                    )}
                    {items.map((path, i) => {
                        const name = basename(path);
                        const dir = dirname(path);
                        return (
                            <button
                                key={path}
                                className={`picker-item${i === sel ? " sel" : ""}`}
                                onMouseEnter={() => {
                                    if (mouseActive.current) setSel(i);
                                }}
                                onClick={() => activate(path)}>
                                <span className="picker-icon project">
                                    <FileIcon name={name} size={14} />
                                </span>
                                <span className="picker-name">{name}</span>
                                <span className="picker-sub">{dir || "."}</span>
                            </button>
                        );
                    })}
                </div>
            </div>
        </div>
    );
}
