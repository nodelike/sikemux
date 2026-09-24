import { useEffect, useMemo, useRef, useState } from "react";
import { useActiveSurfacePane } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { IconSearch, rankBy, useMouseActive } from "../../../plugin-api/ui";
import { BRUNO_CLIENT } from "../kinds";
import type { BruTreeNode, HttpMethod } from "../lib/types";
import { brunoCollectionR, brunoSelectRequest, brunoSettings, closePalettes } from "../state";
import "../bruno.css";

const MAX_RESULTS = 300;

interface ReqRow {
    path: string;
    name: string;
    method: HttpMethod;
    /** breadcrumb of ancestor folders, e.g. "Auth / Tokens" */
    folder: string;
}

function flatten(nodes: BruTreeNode[], trail: string[] = [], acc: ReqRow[] = []): ReqRow[] {
    for (const n of nodes) {
        if (n.type === "folder") flatten(n.children, [...trail, n.name], acc);
        else acc.push({ path: n.path, name: n.name, method: n.method, folder: trail.join(" / ") });
    }
    return acc;
}

export function BrunoRequestPalette() {
    const paneId = useActiveSurfacePane(BRUNO_CLIENT);
    const collectionPath = brunoSettings.useSelect((settings) => settings.collectionPath);

    const [query, setQuery] = useState("");
    const [sel, setSel] = useState(0);
    const inputRef = useRef<HTMLInputElement>(null);
    const listRef = useRef<HTMLDivElement>(null);
    const mouseActive = useMouseActive();

    const coll = useResourceEnabled(!!collectionPath, brunoCollectionR, collectionPath);
    const collection = coll.data;

    useEffect(() => {
        inputRef.current?.focus();
    }, []);

    const all = useMemo(() => (collection ? flatten(collection.tree) : []), [collection]);
    const items = useMemo(
        () => rankBy(query, all, (r) => [r.name, r.folder ? `${r.folder} / ${r.name}` : r.name]).slice(0, MAX_RESULTS),
        [all, query],
    );

    useEffect(() => {
        setSel(0);
    }, [query, items.length]);

    useEffect(() => {
        const el = listRef.current?.querySelector<HTMLElement>(`.picker-item:nth-child(${sel + 1})`);
        el?.scrollIntoView({ block: "nearest" });
    }, [sel]);

    const activate = (row: ReqRow | undefined) => {
        if (!row) return;
        if (paneId) brunoSelectRequest(paneId, row.path);
        closePalettes();
    };

    const onKeyDown = (e: React.KeyboardEvent) => {
        if (e.key === "Escape") {
            closePalettes();
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
        <div className="picker-backdrop" onMouseDown={closePalettes}>
            <div className="picker" onMouseDown={(e) => e.stopPropagation()}>
                <div className="picker-input-wrap">
                    <IconSearch size={15} className="picker-search-icon" />
                    <input
                        ref={inputRef}
                        className="picker-input"
                        placeholder="search requests by name…"
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        onKeyDown={onKeyDown}
                        spellCheck={false}
                    />
                </div>

                <div className="picker-list" ref={listRef}>
                    {items.length === 0 && (
                        <div className="picker-empty">{coll.status === "loading" && !collection ? "loading…" : "no requests"}</div>
                    )}
                    {items.map((row, i) => (
                        <button
                            key={row.path}
                            className={`picker-item${i === sel ? " sel" : ""}`}
                            onMouseEnter={() => {
                                if (mouseActive.current) setSel(i);
                            }}
                            onClick={() => activate(row)}>
                            <span className={`bruno-method m-${row.method}`}>{row.method.toUpperCase()}</span>
                            <span className="picker-name">{row.name}</span>
                            {row.folder && <span className="picker-sub">{row.folder}</span>}
                        </button>
                    ))}
                </div>
            </div>
        </div>
    );
}
