import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Compartment, EditorState, Range as CMRange, StateEffect, StateField } from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, keymap, lineNumbers, type DecorationSet } from "@codemirror/view";
import { syntaxHighlighting, defaultHighlightStyle } from "@codemirror/language";
import { useVirtualizer } from "@tanstack/react-virtual";
import { auraExtensions, languageFor } from "../editor/codemirror";
import { registerView } from "../themes/bus";
import { subscribe } from "../state/bus";
import { searchApi, type SearchFile, type SearchHit, type SearchResults, type ReplaceResults } from "../api/search";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { DEFAULT_GLOBAL_SEARCH_VIEW } from "../state/types";
import { notify, errMessage } from "../state/toast";
import { FileIcon } from "./FileIcon";
import { IconSearch } from "./Icons";
import { basename, dirname, isPathWithin, joinPath, normalizePath } from "../lib/paths";
import { PRIMARY_SHORTCUT, SHIFT_SHORTCUT } from "../lib/platform";

const DEBOUNCE_MS = 250;
const PREVIEW_BEFORE_LINES = 40;
const PREVIEW_AFTER_LINES = 80;
const PREVIEW_LRU_CAP = 4;

type Status = "idle" | "searching" | "ok" | "error";
type SearchBooleanOption = "caseSensitive" | "wholeWord" | "isRegex";

const SEARCH_OPTION_TOGGLES: { key: SearchBooleanOption; label: string; title: string }[] = [
    { key: "caseSensitive", label: "Aa", title: "Match case" },
    { key: "wholeWord", label: "ab", title: "Whole word" },
    { key: "isRegex", label: ".*", title: "Regex" },
];

const SEARCH_SCOPE_FIELDS = [
    { key: "include", label: "in", placeholder: "src/**/*.ts" },
    { key: "exclude", label: "not", placeholder: "**/*.test.ts" },
] as const;

/** `compact` drops the preview column so the pane fits the workspace rail; a
 * result opens in the editor instead of previewing beside its own list. */
export function SearchPane({
    sessionId,
    cwd,
    active,
    visible,
    compact = false,
}: {
    sessionId: string;
    cwd: string;
    active: boolean;
    visible: boolean;
    compact?: boolean;
}) {
    const entry = useStore((s) => s.globalSearchBySession[sessionId]);
    const view = entry ?? DEFAULT_GLOBAL_SEARCH_VIEW;

    const findRef = useRef<HTMLInputElement>(null);
    const replaceRef = useRef<HTMLInputElement>(null);

    const [files, setFiles] = useState<SearchFile[]>([]);
    const [summary, setSummary] = useState<SearchResults | null>(null);
    const [status, setStatus] = useState<Status>("idle");
    const [error, setError] = useState<string | null>(null);
    const [replacing, setReplacing] = useState(false);
    const [replacePreview, setReplacePreview] = useState<ReplaceResults | null>(null);
    const [stale, setStale] = useState(false);

    const requestIdRef = useRef(0);

    const startSearch = useCallback(
        (query: string, options: typeof DEFAULT_GLOBAL_SEARCH_VIEW.options) => {
            const id = ++requestIdRef.current;
            setStatus("searching");
            setFiles([]);
            searchApi
                .project(cwd, query, options, (file) => {
                    if (id !== requestIdRef.current) return;
                    setFiles((prev) => prev.concat(file));
                })
                .then((final) => {
                    if (id !== requestIdRef.current) return;
                    setFiles(final.files);
                    setSummary(final);
                    setStatus("ok");
                    setError(null);
                    setStale(false);
                })
                .catch((e: unknown) => {
                    if (id !== requestIdRef.current) return;
                    setStatus("error");
                    setError(String(e));
                });
        },
        [cwd],
    );

    useEffect(() => {
        if (active) {
            findRef.current?.focus();
            findRef.current?.select();
        }
    }, [active]);

    useEffect(() => {
        if (!sessionId) return;
        return subscribe("search-focus", (e) => {
            if (e.sessionId !== sessionId) return;
            findRef.current?.focus();
            findRef.current?.select();
        });
    }, [sessionId]);

    useEffect(() => {
        const requestId = requestIdRef;
        if (!cwd || !visible) {
            requestId.current++;
            return;
        }
        if (!view.query.trim()) {
            setFiles([]);
            setSummary(null);
            setStatus("idle");
            setError(null);
            void searchApi.cancel(cwd).catch(() => {});
            return;
        }
        setStatus("searching");
        setReplacePreview(null);
        let started = false;
        const handle = window.setTimeout(() => {
            started = true;
            startSearch(view.query, view.options);
        }, DEBOUNCE_MS);
        return () => {
            window.clearTimeout(handle);
            requestId.current++;
            if (started) void searchApi.cancel(cwd).catch(() => {});
        };
    }, [cwd, visible, view.query, view.options, startSearch]);

    useEffect(() => {
        if (files.length === 0) return;
        const sel = view.selected;
        const stillValid = !!sel && files.some((f) => f.path === sel.path && sel.matchIndex < f.matches.length);
        if (!stillValid) {
            cmd.setGlobalSearchSelected(sessionId, {
                path: files[0].path,
                matchIndex: 0,
            });
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [files, sessionId]);

    useEffect(() => {
        setReplacePreview(null);
    }, [view.query, view.replace, view.options]);

    useEffect(() => {
        if (!cwd || !visible) return;
        return subscribe("fs-changed", (e) => {
            if (!e.repo || !isPathWithin(cwd, e.repo)) return;
            if (files.length > 0) setStale(true);
        });
    }, [cwd, visible, files.length]);

    const refresh = useCallback(() => {
        if (!cwd || !view.query.trim()) return;
        startSearch(view.query, view.options);
    }, [cwd, view.query, view.options, startSearch]);

    const runReplaceAll = useCallback(async () => {
        if (!cwd || !view.query.trim() || replacing) return;
        setReplacing(true);
        try {
            if (!replacePreview) {
                const preview = await searchApi.replace(cwd, view.query, view.replace, view.options, true);
                setReplacePreview(preview);
                return;
            }
            const dirty = Object.values(useStore.getState().dirtyEditorPaths).flat();
            const dirtySet = new Set(dirty.map(normalizePath));
            const conflicts = replacePreview.files.map((f) => joinPath(cwd, f.path)).filter((path) => dirtySet.has(normalizePath(path)));
            if (conflicts.length > 0) {
                const shown = conflicts.slice(0, 3).map(basename).join(", ");
                const more = conflicts.length > 3 ? ` and ${conflicts.length - 3} more` : "";
                notify("error", `save or close unsaved files before replace: ${shown}${more}`);
                return;
            }
            const r = await searchApi.replace(cwd, view.query, view.replace, view.options, false);
            const verb = r.match_count === 1 ? "match" : "matches";
            const fileWord = r.file_count === 1 ? "file" : "files";
            notify(
                r.errors.length ? "info" : "success",
                `replaced ${r.match_count} ${verb} in ${r.file_count} ${fileWord}` + (r.errors.length ? ` · ${r.errors.length} skipped` : ""),
            );
            const changed = new Set(r.files.map((f) => f.path));
            setFiles((prev) => prev.filter((f) => !changed.has(f.path)));
            setSummary((prev) =>
                prev
                    ? {
                          ...prev,
                          files: prev.files.filter((f) => !changed.has(f.path)),
                          file_count: Math.max(0, prev.file_count - r.files.length),
                          match_count: Math.max(0, prev.match_count - r.match_count),
                      }
                    : prev,
            );
            setReplacePreview(null);
        } catch (e) {
            notify("error", `replace failed: ${errMessage(e)}`);
            setReplacePreview(null);
        } finally {
            setReplacing(false);
        }
    }, [cwd, view.query, view.replace, view.options, replacing, replacePreview]);

    const results: SearchResults | null = useMemo(() => {
        if (!summary && files.length === 0) return null;
        return {
            files,
            file_count: summary?.file_count ?? files.length,
            match_count: summary?.match_count ?? files.reduce((n, f) => n + f.matches.length, 0),
            truncated: summary?.truncated ?? false,
            cancelled: summary?.cancelled,
            elapsed_ms: summary?.elapsed_ms ?? 0,
        };
    }, [summary, files]);

    return (
        <div className={`search-pane${compact ? " compact" : ""}`}>
            <div className="sp-body">
                <div className="sp-left">
                    <Header
                        sessionId={sessionId}
                        view={view}
                        findRef={findRef}
                        replaceRef={replaceRef}
                        status={status}
                        results={results}
                        replacing={replacing}
                        replacePreview={replacePreview}
                        stale={stale}
                        onReplaceAll={runReplaceAll}
                        onRefresh={refresh}
                    />
                    <Threads
                        sessionId={sessionId}
                        cwd={cwd}
                        query={view.query}
                        replace={view.replace}
                        selected={view.selected}
                        collapsed={view.collapsed}
                        results={results}
                        status={status}
                        error={error}
                    />
                </div>
                {!compact && (
                    <PreviewArea repo={cwd} query={view.query} replace={view.replace} results={results} status={status} selected={view.selected} />
                )}
            </div>
            {!compact && <Footer />}
        </div>
    );
}

function Header({
    sessionId,
    view,
    findRef,
    replaceRef,
    status,
    results,
    replacing,
    replacePreview,
    stale,
    onReplaceAll,
    onRefresh,
}: {
    sessionId: string;
    view: typeof DEFAULT_GLOBAL_SEARCH_VIEW;
    findRef: React.RefObject<HTMLInputElement | null>;
    replaceRef: React.RefObject<HTMLInputElement | null>;
    status: Status;
    results: SearchResults | null;
    replacing: boolean;
    replacePreview: ReplaceResults | null;
    stale: boolean;
    onReplaceAll: () => void;
    onRefresh: () => void;
}) {
    const { query, replace, replaceOpen, options } = view;
    const hasResults = !!results && results.match_count > 0;
    const canReplace = hasResults && !replacing;
    const openReplaceAndFocus = () => {
        if (!replaceOpen) cmd.toggleGlobalSearchReplaceOpen(sessionId);
        window.setTimeout(() => replaceRef.current?.focus(), 0);
    };
    const [scopeOpen, setScopeOpen] = useState(!!options.include || !!options.exclude);

    return (
        <div className="sp-head">
            <div className="sp-row find">
                <button
                    type="button"
                    className={`sp-chev${replaceOpen ? " open" : ""}`}
                    onClick={() => cmd.toggleGlobalSearchReplaceOpen(sessionId)}
                    title={replaceOpen ? "Hide replace" : "Show replace"}
                    aria-expanded={replaceOpen}>
                    ▸
                </button>
                <span className="sp-av find" aria-hidden>
                    <IconSearch size={11} />
                </span>
                <input
                    ref={findRef}
                    className="sp-input"
                    placeholder="find in project"
                    value={query}
                    onChange={(e) => cmd.setGlobalSearchQuery(sessionId, e.target.value)}
                    onKeyDown={(e) => {
                        if (e.key === "Tab" && !e.shiftKey) {
                            e.preventDefault();
                            openReplaceAndFocus();
                        }
                    }}
                    spellCheck={false}
                />
                {status === "searching" && <span className="sp-spinner" aria-hidden />}
                <div className="sp-row-toggles">
                    {SEARCH_OPTION_TOGGLES.map((t) => (
                        <Toggle
                            key={t.key}
                            label={t.label}
                            title={t.title}
                            active={options[t.key]}
                            onClick={() => cmd.setGlobalSearchOption(sessionId, t.key, !options[t.key])}
                        />
                    ))}
                </div>
            </div>

            {replaceOpen && (
                <div className="sp-row repl">
                    <span className="sp-chev-spacer" aria-hidden />
                    <span className="sp-av repl" aria-hidden>
                        ↻
                    </span>
                    <input
                        ref={replaceRef}
                        className="sp-input"
                        placeholder="replace with…"
                        value={replace}
                        onChange={(e) => cmd.setGlobalSearchReplace(sessionId, e.target.value)}
                        onKeyDown={(e) => {
                            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                                e.preventDefault();
                                if (canReplace) onReplaceAll();
                            }
                        }}
                        spellCheck={false}
                    />
                </div>
            )}

            {scopeOpen && (
                <>
                    {SEARCH_SCOPE_FIELDS.map((f) => (
                        <div key={f.key} className="sp-row scope">
                            <FilterField
                                label={f.label}
                                placeholder={f.placeholder}
                                value={options[f.key]}
                                onChange={(v) => cmd.setGlobalSearchOption(sessionId, f.key, v)}
                            />
                        </div>
                    ))}
                </>
            )}

            <div className="sp-row actions">
                <div className="sp-toggles">
                    <Toggle
                        label="…"
                        title={scopeOpen ? "Hide file scope" : "Files to include / exclude"}
                        active={scopeOpen}
                        onClick={() => setScopeOpen((v) => !v)}
                    />
                </div>
                <span className="sp-stats">
                    {stale && (
                        <button type="button" className="sp-stale" onClick={onRefresh} title="Files changed on disk — rerun the search">
                            stale ↻
                        </button>
                    )}
                    {results ? (
                        <>
                            <strong>{results.match_count}</strong>
                            <span className="sp-stats-sep">/</span>
                            <strong>{results.file_count}</strong>
                            <span className="sp-stats-time">{results.elapsed_ms}ms</span>
                            {results.truncated && <span className="sp-truncated">trunc</span>}
                            {results.cancelled && <span className="sp-truncated">partial</span>}
                        </>
                    ) : (
                        <span className="sp-stats-dim">·</span>
                    )}
                </span>
                {replaceOpen && (
                    <button
                        type="button"
                        className={`sp-replace-btn${replacePreview ? " sp-replace-btn-commit" : ""}`}
                        onClick={onReplaceAll}
                        disabled={!canReplace}
                        title={
                            replacePreview
                                ? `Commit ${replacePreview.match_count} replacements across ${replacePreview.file_count} files`
                                : canReplace
                                  ? "Preview replacements"
                                  : "Run a search first"
                        }>
                        {replacing
                            ? "…"
                            : replacePreview
                              ? `commit ${replacePreview.match_count}`
                              : `preview${results && results.match_count > 0 ? ` ${results.match_count}` : ""}`}
                    </button>
                )}
            </div>
        </div>
    );
}

function Toggle({ label, title, active, onClick }: { label: string; title: string; active: boolean; onClick: () => void }) {
    return (
        <button className={`sp-toggle${active ? " on" : ""}`} onClick={onClick} title={title} type="button">
            {label}
        </button>
    );
}

function FilterField({ label, placeholder, value, onChange }: { label: string; placeholder: string; value: string; onChange: (v: string) => void }) {
    return (
        <label className="sp-filter">
            <span className="sp-filter-label">{label}</span>
            <input
                className="sp-filter-input"
                placeholder={placeholder}
                value={value}
                onChange={(e) => onChange(e.target.value)}
                spellCheck={false}
            />
        </label>
    );
}

type Row =
    | { kind: "header"; file: SearchFile; index: number; collapsed: boolean }
    | { kind: "msg"; file: SearchFile; hit: SearchHit; hitIndex: number; index: number };

const HEADER_ROW_HEIGHT = 24;
const MSG_ROW_HEIGHT = 20;

function ThreadNotice({ className, children }: { className: string; children: React.ReactNode }) {
    return (
        <div className="sp-threads-wrap">
            <div className={className}>{children}</div>
        </div>
    );
}

function Threads({
    sessionId,
    cwd,
    query,
    replace,
    selected,
    collapsed,
    results,
    status,
    error,
}: {
    sessionId: string;
    cwd: string;
    query: string;
    replace: string;
    selected: typeof DEFAULT_GLOBAL_SEARCH_VIEW.selected;
    collapsed: Record<string, boolean>;
    results: SearchResults | null;
    status: Status;
    error: string | null;
}) {
    const scrollRef = useRef<HTMLDivElement>(null);

    const rows: Row[] = useMemo(() => {
        if (!results) return [];
        const out: Row[] = [];
        let i = 0;
        for (const file of results.files) {
            const isCollapsed = !!collapsed[file.path];
            out.push({ kind: "header", file, collapsed: isCollapsed, index: i++ });
            if (isCollapsed) continue;
            for (let hi = 0; hi < file.matches.length; hi++) out.push({ kind: "msg", file, hit: file.matches[hi], hitIndex: hi, index: i++ });
        }
        return out;
    }, [results, collapsed]);

    const virtualizer = useVirtualizer({
        count: rows.length,
        getScrollElement: () => scrollRef.current,
        estimateSize: (i) => (rows[i]?.kind === "header" ? HEADER_ROW_HEIGHT : MSG_ROW_HEIGHT),
        overscan: 12,
        getItemKey: (i) => {
            const r = rows[i];
            if (!r) return i;
            return r.kind === "header" ? `h:${r.file.path}` : `m:${r.file.path}:${r.hitIndex}`;
        },
    });

    useEffect(() => {
        if (!selected || rows.length === 0) return;
        const idx = rows.findIndex((r) => r.kind === "msg" && r.file.path === selected.path && r.hitIndex === selected.matchIndex);
        if (idx >= 0) virtualizer.scrollToIndex(idx, { align: "auto" });
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [selected]);

    if (!cwd) return <ThreadNotice className="sp-empty">open a project session</ThreadNotice>;
    if (status === "error") {
        return <ThreadNotice className="sp-err">{error?.includes("invalid") ? `invalid regex: ${error}` : (error ?? "search failed")}</ThreadNotice>;
    }
    if (!query.trim()) {
        return (
            <ThreadNotice className="sp-empty">
                start typing to search
                <span className="sp-empty-sub">tab → replace · {PRIMARY_SHORTCUT}↵ replace all</span>
            </ThreadNotice>
        );
    }
    if (status === "searching" && rows.length === 0) return <ThreadNotice className="sp-loading">searching…</ThreadNotice>;
    if (rows.length === 0) return <ThreadNotice className="sp-empty">no matches</ThreadNotice>;

    const items = virtualizer.getVirtualItems();
    return (
        <div className="sp-threads" ref={scrollRef}>
            <div className="sp-threads-spacer" style={{ height: virtualizer.getTotalSize() }}>
                {items.map((vi) => {
                    const row = rows[vi.index];
                    if (!row) return null;
                    const style: React.CSSProperties = {
                        position: "absolute",
                        top: 0,
                        left: 0,
                        right: 0,
                        transform: `translateY(${vi.start}px)`,
                    };
                    if (row.kind === "header") {
                        return <FileHeader key={vi.key} style={style} sessionId={sessionId} file={row.file} collapsed={row.collapsed} />;
                    }
                    return (
                        <MessageRow
                            key={vi.key}
                            style={style}
                            sessionId={sessionId}
                            repo={cwd}
                            file={row.file}
                            hit={row.hit}
                            hitIndex={row.hitIndex}
                            isSelected={selected?.path === row.file.path && selected.matchIndex === row.hitIndex}
                            replace={replace}
                        />
                    );
                })}
            </div>
        </div>
    );
}

const FileHeader = memo(function FileHeader({
    style,
    sessionId,
    file,
    collapsed,
}: {
    style: React.CSSProperties;
    sessionId: string;
    file: SearchFile;
    collapsed: boolean;
}) {
    const name = basename(file.path);
    const dir = dirname(file.path);
    return (
        <button
            type="button"
            className="sp-thread-who"
            style={style}
            onClick={() => cmd.toggleGlobalSearchFileCollapsed(sessionId, file.path)}
            aria-expanded={!collapsed}
            title={collapsed ? "Expand file" : "Collapse file"}>
            <span className={`sp-thread-chev${collapsed ? "" : " open"}`}>▸</span>
            <FileIcon name={name} size={13} />
            <span className="sp-thread-name">{name}</span>
            {dir && <span className="sp-thread-dir">{dir}/</span>}
            <span className="sp-thread-count">{file.matches.length}</span>
        </button>
    );
});

const MessageRow = memo(function MessageRow({
    style,
    sessionId,
    repo,
    file,
    hit,
    hitIndex,
    isSelected,
    replace,
}: {
    style: React.CSSProperties;
    sessionId: string;
    repo: string;
    file: SearchFile;
    hit: SearchHit;
    hitIndex: number;
    isSelected: boolean;
    replace: string;
}) {
    return (
        <button
            type="button"
            className={`sp-msg${isSelected ? " sel" : ""}`}
            style={style}
            onClick={() => cmd.setGlobalSearchSelected(sessionId, { path: file.path, matchIndex: hitIndex })}
            onDoubleClick={() => cmd.requestOpenFile(joinPath(repo, file.path), hit.line - 1, hit.ranges[0]?.start ?? 0)}
            title={replace ? `${hit.text}  →  (with replace)` : hit.text}>
            <span className="sp-msg-ln">{hit.line}</span>
            <span className="sp-msg-tx">
                <HighlightedLine hit={hit} replace={replace} />
            </span>
        </button>
    );
});

const HighlightedLine = memo(function HighlightedLine({ hit, replace }: { hit: SearchHit; replace: string }) {
    const ranges = hit.ranges;
    if (ranges.length === 0) return <>{hit.text}</>;
    const parts: React.ReactNode[] = [];
    let cursor = 0;
    ranges.forEach((r, i) => {
        if (r.start > cursor) parts.push(hit.text.slice(cursor, r.start));
        parts.push(<mark key={`m${i}`}>{hit.text.slice(r.start, r.end)}</mark>);
        if (replace && i === 0) {
            parts.push(
                <span key={`r${i}`} className="sp-msg-repl">
                    {" → "}
                    {replace}
                </span>,
            );
        }
        cursor = r.end;
    });
    if (cursor < hit.text.length) parts.push(hit.text.slice(cursor));
    return <>{parts}</>;
});

const setSelectedLineEffect = StateEffect.define<{
    line: number;
    ranges: { start: number; end: number }[];
}>();

const hitLineMark = Decoration.line({ class: "cm-sp-hit-line" });
const hitMatchMark = Decoration.mark({ class: "cm-sp-hit-match" });

function previewDecorations(state: EditorState, payload: { line: number; ranges: { start: number; end: number }[] } | null): DecorationSet {
    if (!payload || payload.line < 1 || payload.line > state.doc.lines) {
        return Decoration.none;
    }
    const line = state.doc.line(payload.line);
    const decos: CMRange<Decoration>[] = [];
    decos.push(hitLineMark.range(line.from));
    for (const r of payload.ranges) {
        const from = Math.min(line.from + r.start, line.to);
        const to = Math.min(line.from + r.end, line.to);
        if (to > from) decos.push(hitMatchMark.range(from, to));
    }
    return Decoration.set(decos, true);
}

function previewDecorationField() {
    return StateField.define<DecorationSet>({
        create: () => Decoration.none,
        update(value, tr) {
            let next: { line: number; ranges: { start: number; end: number }[] } | null = null;
            let touched = false;
            for (const e of tr.effects) {
                if (e.is(setSelectedLineEffect)) {
                    next = e.value;
                    touched = true;
                }
            }
            if (touched) return previewDecorations(tr.state, next);
            if (tr.docChanged) return Decoration.none;
            return value;
        },
        provide: (f) => EditorView.decorations.from(f),
    });
}

function previewDoubleClickPlugin(onOpenRef: React.RefObject<() => void>) {
    return ViewPlugin.define(() => ({}), {
        eventHandlers: {
            dblclick: () => {
                onOpenRef.current();
                return false;
            },
        },
    });
}

function previewKeymap(onOpenRef: React.RefObject<() => void>) {
    return keymap.of([
        {
            key: "Enter",
            preventDefault: true,
            run: () => {
                onOpenRef.current();
                return true;
            },
        },
    ]);
}

function PreviewNotice({ className, children }: { className: string; children: React.ReactNode }) {
    return (
        <div className="sp-preview">
            <div className={className}>{children}</div>
        </div>
    );
}

function PreviewArea({
    repo,
    query,
    replace,
    results,
    status,
    selected,
}: {
    repo: string;
    query: string;
    replace: string;
    results: SearchResults | null;
    status: Status;
    selected: typeof DEFAULT_GLOBAL_SEARCH_VIEW.selected;
}) {
    if (!repo) return <PreviewNotice className="sp-preview-empty">no project open</PreviewNotice>;
    if (!query.trim()) return <PreviewNotice className="sp-preview-empty">preview will appear here</PreviewNotice>;
    if (status === "searching" && (!results || results.files.length === 0))
        return <PreviewNotice className="sp-preview-loading">searching…</PreviewNotice>;
    if (!results || results.files.length === 0) return <PreviewNotice className="sp-preview-empty">nothing to preview</PreviewNotice>;
    return <Preview repo={repo} results={results} selected={selected} replace={replace} />;
}

interface WindowEntry {
    doc: string;
    startLine: number;
}

function Preview({
    repo,
    results,
    selected,
    replace,
}: {
    repo: string;
    results: SearchResults;
    selected: typeof DEFAULT_GLOBAL_SEARCH_VIEW.selected;
    replace: string;
}) {
    const hostRef = useRef<HTMLDivElement>(null);
    const viewRef = useRef<EditorView | null>(null);
    const languageCompRef = useRef<Compartment>(new Compartment());
    const lineNumberCompRef = useRef<Compartment>(new Compartment());
    const cacheRef = useRef<Map<string, WindowEntry>>(new Map());
    const [active, setActive] = useState<{
        path: string;
        entry: WindowEntry;
    } | null>(null);
    const [err, setErr] = useState<string | null>(null);
    const onOpenRef = useRef<() => void>(() => {});

    const resolved = useMemo(() => {
        if (!results || results.files.length === 0) return null;
        if (selected) {
            const file = results.files.find((f) => f.path === selected.path);
            if (file && file.matches[selected.matchIndex]) {
                return { file, hit: file.matches[selected.matchIndex] };
            }
        }
        const first = results.files[0];
        return { file: first, hit: first.matches[0] };
    }, [results, selected]);

    useEffect(() => {
        onOpenRef.current = () => {
            if (!resolved) return;
            const hit = resolved.hit;
            cmd.requestOpenFile(joinPath(repo, resolved.file.path), hit.line - 1, hit.ranges[0]?.start ?? 0);
        };
    }, [repo, resolved]);

    useEffect(() => {
        if (!resolved) return;
        const wantedPath = joinPath(repo, resolved.file.path);
        const cached = cacheRef.current.get(wantedPath);
        const hitLine = resolved.hit.line;
        const inWindow = !!cached && hitLine >= cached.startLine && hitLine < cached.startLine + countLines(cached.doc);
        if (inWindow && cached) {
            bumpLru(cacheRef.current, wantedPath, cached);
            setActive({ path: wantedPath, entry: cached });
            setErr(null);
            return;
        }
        let cancelled = false;
        searchApi
            .readFileWindow(wantedPath, hitLine, PREVIEW_BEFORE_LINES, PREVIEW_AFTER_LINES)
            .then((win) => {
                if (cancelled) return;
                const entry: WindowEntry = {
                    doc: win.doc,
                    startLine: win.start_line,
                };
                putLru(cacheRef.current, wantedPath, entry);
                setActive({ path: wantedPath, entry });
                setErr(null);
            })
            .catch((e: unknown) => {
                if (cancelled) return;
                setErr(errMessage(e));
                setActive(null);
            });
        return () => {
            cancelled = true;
        };
    }, [repo, resolved]);

    useEffect(() => {
        if (!hostRef.current) return;
        const exts = [
            EditorState.readOnly.of(true),
            EditorView.editable.of(false),
            EditorView.contentAttributes.of({ tabindex: "0" }),
            auraExtensions,
            languageCompRef.current.of([]),
            syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
            lineNumberCompRef.current.of(lineNumbers()),
            previewDecorationField(),
            previewDoubleClickPlugin(onOpenRef),
            previewKeymap(onOpenRef),
            EditorView.theme({
                "&": { background: "transparent", fontSize: "12px" },
                ".cm-scroller": {
                    lineHeight: "1.65",
                    fontFamily: "var(--mono)",
                },
                ".cm-gutters": {
                    background: "transparent",
                    borderRight: "none",
                    color: "var(--ink-faint)",
                    paddingRight: "10px",
                },
                ".cm-lineNumbers .cm-gutterElement": {
                    padding: "0 6px",
                    minWidth: "38px",
                    textAlign: "right",
                    fontVariantNumeric: "tabular-nums",
                    fontSize: "11px",
                },
                ".cm-content": { padding: "8px 0" },
                ".cm-line": { padding: "0 14px" },
                "&.cm-focused": { outline: "none" },
            }),
            EditorView.domEventHandlers({
                keydown: (e) => {
                    if (e.key === "Escape") return false;
                    e.stopPropagation();
                    return false;
                },
            }),
        ];
        const state = EditorState.create({ extensions: exts });
        const view = new EditorView({ parent: hostRef.current, state });
        viewRef.current = view;
        const unreg = registerView(view);
        return () => {
            unreg();
            view.destroy();
            viewRef.current = null;
        };
    }, []);

    useEffect(() => {
        const view = viewRef.current;
        if (!view || !active || !resolved) return;
        const languageExts = languageFor(resolved.file.path);
        const startLine = active.entry.startLine;
        const lineNoInWindow = Math.max(1, resolved.hit.line - startLine + 1);
        view.dispatch({
            changes: {
                from: 0,
                to: view.state.doc.length,
                insert: active.entry.doc,
            },
            effects: [
                languageCompRef.current.reconfigure(languageExts),
                lineNumberCompRef.current.reconfigure(
                    lineNumbers({
                        formatNumber: (n) => String(startLine + n - 1),
                    }),
                ),
                setSelectedLineEffect.of({
                    line: lineNoInWindow,
                    ranges: resolved.hit.ranges,
                }),
            ],
        });
        requestAnimationFrame(() => {
            const v = viewRef.current;
            if (!v) return;
            if (lineNoInWindow < 1 || lineNoInWindow > v.state.doc.lines) return;
            const line = v.state.doc.line(lineNoInWindow);
            v.dispatch({ effects: EditorView.scrollIntoView(line.from, { y: "center" }) });
        });
    }, [active, resolved]);

    if (!resolved) {
        return (
            <div className="sp-preview">
                <div className="sp-preview-empty">no match selected</div>
            </div>
        );
    }

    const { file, hit } = resolved;
    const total = file.matches.length;
    const currentIndex = file.matches.findIndex((m) => m === hit) + 1;

    return (
        <div className="sp-preview">
            <button type="button" className="sp-preview-head" onClick={onOpenRef.current} title={`Open ${file.path} at line ${hit.line}`}>
                <FileIcon name={basename(file.path)} size={13} />
                <span className="sp-preview-name">{basename(file.path)}</span>
                <span className="sp-preview-dir">{dirname(file.path)}/</span>
                <span className="sp-preview-meta">
                    line {hit.line} · {currentIndex} of {total}
                    {replace && <span className="sp-preview-repl"> → {replace}</span>}
                </span>
                <span className="sp-preview-open" aria-hidden>
                    ↗
                </span>
            </button>
            <div className="sp-preview-body">
                <div className="sp-preview-host" ref={hostRef} />
                {err && (
                    <div className="sp-preview-overlay sp-preview-err">
                        could not load {file.path}: {err}
                    </div>
                )}
                {!active && !err && <div className="sp-preview-overlay sp-preview-loading">loading…</div>}
            </div>
        </div>
    );
}

function countLines(s: string): number {
    if (s.length === 0) return 1;
    let n = 1;
    for (let i = 0; i < s.length; i++) if (s.charCodeAt(i) === 10) n++;
    return n;
}

function bumpLru(m: Map<string, WindowEntry>, key: string, v: WindowEntry): void {
    m.delete(key);
    m.set(key, v);
}

function putLru(m: Map<string, WindowEntry>, key: string, v: WindowEntry): void {
    m.delete(key);
    m.set(key, v);
    while (m.size > PREVIEW_LRU_CAP) {
        const first = m.keys().next().value;
        if (first === undefined) break;
        m.delete(first);
    }
}

function Footer() {
    return (
        <div className="sp-foot">
            <span>
                <kbd>↵</kbd> open
            </span>
            <span>
                <kbd>tab</kbd> replace
            </span>
            <span>
                <kbd>{PRIMARY_SHORTCUT}↵</kbd> replace all
            </span>
            <span>
                <kbd>
                    {PRIMARY_SHORTCUT}
                    {SHIFT_SHORTCUT}F
                </kbd>{" "}
                focus
            </span>
        </div>
    );
}
