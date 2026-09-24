import { useEffect, useMemo, useRef, useState } from "react";
import { useActiveSurfacePane } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { IconCheck, IconSearch, rankBy, useMouseActive } from "../../../plugin-api/ui";
import { BRUNO_CLIENT } from "../kinds";
import { findRequest } from "../lib/resolve";
import { brunoCollectionR, brunoSelectEnv, brunoSettings, closePalettes, useBrunoView } from "../state";
import "../bruno.css";

const NO_ENV = "__none__";

interface EnvRow {
    /** null = clear the environment */
    id: string | null;
    name: string;
    collectionName: string;
    showCollection: boolean;
}

export function BrunoEnvPalette() {
    const paneId = useActiveSurfacePane(BRUNO_CLIENT);
    const collectionPath = brunoSettings.useSelect((settings) => settings.collectionPath);
    const selectedEnvs = brunoSettings.useSelect((settings) => settings.selectedEnvs);
    const view = useBrunoView(paneId ?? "");

    const [query, setQuery] = useState("");
    const [sel, setSel] = useState(0);
    const inputRef = useRef<HTMLInputElement>(null);
    const listRef = useRef<HTMLDivElement>(null);
    const mouseActive = useMouseActive();

    const coll = useResourceEnabled(!!collectionPath, brunoCollectionR, collectionPath);
    const collection = coll.data;

    // Scope environments to the open request's collection, matching the header picker.
    const reqCollPath = useMemo(() => {
        if (!collection || !view.activeRequestPath) return "";
        return findRequest(collection.tree, view.activeRequestPath)?.collectionPath ?? "";
    }, [collection, view.activeRequestPath]);

    const visibleEnvs = useMemo(() => {
        if (!collection) return [];
        return reqCollPath ? collection.envs.filter((e) => e.collectionPath === reqCollPath) : collection.envs;
    }, [collection, reqCollPath]);
    const showCollection = !reqCollPath; // disambiguate by collection only when not scoped
    const selectedEnvId = selectedEnvs[reqCollPath] ?? null;

    const rows = useMemo<EnvRow[]>(
        () => [
            { id: null, name: "No environment", collectionName: "", showCollection: false },
            ...visibleEnvs.map((e) => ({ id: e.id, name: e.name, collectionName: e.collectionName, showCollection })),
        ],
        [visibleEnvs, showCollection],
    );

    const items = useMemo(() => rankBy(query, rows, (r) => [r.name, r.showCollection ? `${r.collectionName} ${r.name}` : r.name]), [rows, query]);

    useEffect(() => {
        inputRef.current?.focus();
    }, []);

    // Once the collection loads, park the cursor on the currently-selected env.
    useEffect(() => {
        if (!collection) return;
        const idx = rows.findIndex((r) => r.id === selectedEnvId);
        setSel(idx > 0 ? idx : 0);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [collection]);

    useEffect(() => {
        const el = listRef.current?.querySelector<HTMLElement>(`.picker-item:nth-child(${sel + 1})`);
        el?.scrollIntoView({ block: "nearest" });
    }, [sel]);

    const activate = (row: EnvRow | undefined) => {
        if (!row) return;
        brunoSelectEnv(reqCollPath, row.id);
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
                        placeholder="select environment…"
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        onKeyDown={onKeyDown}
                        spellCheck={false}
                    />
                </div>

                <div className="picker-list" ref={listRef}>
                    {items.length === 0 && (
                        <div className="picker-empty">{coll.status === "loading" && !collection ? "loading…" : "no environments"}</div>
                    )}
                    {items.map((row, i) => {
                        const current = row.id === selectedEnvId;
                        return (
                            <button
                                key={row.id ?? NO_ENV}
                                className={`picker-item${i === sel ? " sel" : ""}`}
                                onMouseEnter={() => {
                                    if (mouseActive.current) setSel(i);
                                }}
                                onClick={() => activate(row)}>
                                <span className={`bruno-env-dot${row.id ? "" : " none"}`} />
                                <span className="picker-name">{row.name}</span>
                                {row.showCollection && row.id && <span className="picker-sub">{row.collectionName}</span>}
                                {current && <IconCheck size={13} className="picker-check" />}
                            </button>
                        );
                    })}
                </div>
            </div>
        </div>
    );
}
