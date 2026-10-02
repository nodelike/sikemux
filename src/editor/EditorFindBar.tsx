import { useEffect, useMemo, useRef, useState } from "react";
import { EditorView } from "@codemirror/view";
import { SearchQuery, findNext, findPrevious, getSearchQuery, replaceAll, replaceNext, setSearchQuery } from "@codemirror/search";
import { IconArrowDown, IconArrowUp, IconChevron, IconClose, IconReplace, IconReplaceAll, IconSearch } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";
import { PRIMARY_SHORTCUT } from "../lib/platform";

interface Props {
    getView: () => EditorView | null;
    /** Changes when the pane swaps documents, so the query is re-applied to the new state. */
    documentKey: string | null;
    open: boolean;
    replaceOpenOnMount: boolean;
    seed: string | null;
    signal: number;
    onClose: () => void;
}

const MAX_COUNTED_MATCHES = 1_000;

export function EditorFindBar({ getView, documentKey, open, replaceOpenOnMount, seed, signal, onClose }: Props) {
    const viewGetter = useRef(getView);
    viewGetter.current = getView;
    const [query, setQuery] = useState("");
    const [replace, setReplace] = useState("");
    const [caseSensitive, setCaseSensitive] = useState(false);
    const [wholeWord, setWholeWord] = useState(false);
    const [regexp, setRegexp] = useState(false);
    const [replaceOpen, setReplaceOpen] = useState(replaceOpenOnMount);
    const [counts, setCounts] = useState<{ total: number; current: number }>({
        total: 0,
        current: 0,
    });

    const findInputRef = useRef<HTMLInputElement | null>(null);
    const replaceInputRef = useRef<HTMLInputElement | null>(null);

    useEffect(() => {
        if (open && replaceOpenOnMount) setReplaceOpen(true);
    }, [open, replaceOpenOnMount]);

    useEffect(() => {
        if (!open) return;
        const view = viewGetter.current();
        if (!view) return;
        const q = new SearchQuery({
            search: query,
            replace,
            caseSensitive,
            regexp,
            wholeWord,
        });
        view.dispatch({ effects: setSearchQuery.of(q) });
        setCounts(computeCounts(view, q));
    }, [open, documentKey, query, replace, caseSensitive, regexp, wholeWord]);

    useEffect(() => {
        if (!open) return;
        const view = viewGetter.current();
        if (!view) return;
        if (!query) {
            setCounts({ total: 0, current: 0 });
            return;
        }
        let raf = 0;
        const tick = () => {
            const q = getSearchQuery(view.state);
            if (q.search) setCounts(computeCounts(view, q));
        };
        raf = window.requestAnimationFrame(tick);
        return () => window.cancelAnimationFrame(raf);
    }, [open, query, replaceOpen]);

    useEffect(() => {
        if (!open) return;
        if (seed && seed.length > 0) setQuery(seed);
        const t = window.setTimeout(() => {
            findInputRef.current?.focus();
            findInputRef.current?.select();
        }, 0);
        return () => window.clearTimeout(t);
        // eslint-disable-next-line react-hooks/exhaustive-deps -- the seed is applied only when the bar is opened or summoned again
    }, [open, signal]);

    const run = (fn: (view: EditorView) => boolean, center = true) => {
        const view = viewGetter.current();
        if (!view) return;
        fn(view);
        if (center) {
            const sel = view.state.selection.main;
            view.dispatch({
                effects: EditorView.scrollIntoView(sel.from, { y: "center" }),
            });
        }
        const q = getSearchQuery(view.state);
        if (q.search) setCounts(computeCounts(view, q));
        view.focus();
    };

    const onFindKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
        if (e.key === "Enter") {
            e.preventDefault();
            run(e.shiftKey ? findPrevious : findNext);
        } else if (e.key === "Escape") {
            e.preventDefault();
            onClose();
            getView()?.focus();
        }
    };

    const onReplaceKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
        if (e.key === "Enter") {
            e.preventDefault();
            run(e.metaKey || e.ctrlKey ? replaceAll : replaceNext);
        } else if (e.key === "Escape") {
            e.preventDefault();
            onClose();
            getView()?.focus();
        }
    };

    const status = useMemo(() => {
        if (!query) return "";
        if (counts.total === 0) return "No results";
        const total = counts.total >= MAX_COUNTED_MATCHES ? `${MAX_COUNTED_MATCHES}+` : `${counts.total}`;
        return `${counts.current} of ${total}`;
    }, [query, counts]);

    if (!open) return null;

    return (
        <div className={`ed-findbar${replaceOpen ? " replace" : ""}`}>
            <button
                type="button"
                className="ed-findbar-chev"
                onClick={() => {
                    const next = !replaceOpen;
                    setReplaceOpen(next);
                    if (next) {
                        window.setTimeout(() => replaceInputRef.current?.focus(), 0);
                    }
                }}
                aria-label={replaceOpen ? "Hide replace" : "Show replace"}
                aria-expanded={replaceOpen}>
                <IconChevron size={10} />
            </button>

            <div className="ed-findbar-rows">
                <div className="ed-findbar-row">
                    <span className="ed-findbar-av find" aria-hidden>
                        <IconSearch size={11} />
                    </span>
                    <input
                        ref={findInputRef}
                        className="ed-findbar-input"
                        placeholder="Find"
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        onKeyDown={onFindKeyDown}
                        spellCheck={false}
                    />
                    <div className="ed-findbar-toggles">
                        <Tooltip label="Match case">
                            <button
                                type="button"
                                className={`ed-findbar-toggle${caseSensitive ? " on" : ""}`}
                                aria-label="Match case"
                                aria-pressed={caseSensitive}
                                onClick={() => setCaseSensitive((v) => !v)}>
                                Aa
                            </button>
                        </Tooltip>
                        <Tooltip label="Whole word">
                            <button
                                type="button"
                                className={`ed-findbar-toggle${wholeWord ? " on" : ""}`}
                                aria-label="Whole word"
                                aria-pressed={wholeWord}
                                onClick={() => setWholeWord((v) => !v)}>
                                ab
                            </button>
                        </Tooltip>
                        <Tooltip label="Regex">
                            <button
                                type="button"
                                className={`ed-findbar-toggle${regexp ? " on" : ""}`}
                                aria-label="Regular expression"
                                aria-pressed={regexp}
                                onClick={() => setRegexp((v) => !v)}>
                                .*
                            </button>
                        </Tooltip>
                    </div>
                    <span className="ed-findbar-status">{status}</span>
                    <Tooltip label="Previous — ⇧⏎">
                        <button type="button" className="ed-findbar-btn" aria-label="Previous match" onClick={() => run(findPrevious)}>
                            <IconArrowUp size={12} />
                        </button>
                    </Tooltip>
                    <Tooltip label="Next — ⏎">
                        <button type="button" className="ed-findbar-btn" aria-label="Next match" onClick={() => run(findNext)}>
                            <IconArrowDown size={12} />
                        </button>
                    </Tooltip>
                    <Tooltip label="Close — esc">
                        <button
                            type="button"
                            className="ed-findbar-btn close"
                            aria-label="Close find bar"
                            onClick={() => {
                                onClose();
                                getView()?.focus();
                            }}>
                            <IconClose size={11} />
                        </button>
                    </Tooltip>
                </div>

                {replaceOpen && (
                    <div className="ed-findbar-row">
                        <span className="ed-findbar-av repl" aria-hidden>
                            <IconReplace size={11} />
                        </span>
                        <input
                            ref={replaceInputRef}
                            className="ed-findbar-input"
                            placeholder="Replace"
                            value={replace}
                            onChange={(e) => setReplace(e.target.value)}
                            onKeyDown={onReplaceKeyDown}
                            spellCheck={false}
                        />
                        <span className="ed-findbar-status" aria-hidden />
                        <Tooltip label="Replace — ⏎">
                            <button
                                type="button"
                                className="ed-findbar-btn"
                                aria-label="Replace match"
                                onClick={() => run(replaceNext)}
                                disabled={!query}>
                                <IconReplace size={12} />
                            </button>
                        </Tooltip>
                        <Tooltip label={`Replace all — ${PRIMARY_SHORTCUT}⏎`}>
                            <button
                                type="button"
                                className="ed-findbar-btn"
                                aria-label="Replace all matches"
                                onClick={() => run(replaceAll)}
                                disabled={!query}>
                                <IconReplaceAll size={12} />
                            </button>
                        </Tooltip>
                        <span className="ed-findbar-btn-spacer" aria-hidden />
                    </div>
                )}
            </div>
        </div>
    );
}

function computeCounts(view: EditorView, q: SearchQuery): { total: number; current: number } {
    if (!q.valid || !q.search) return { total: 0, current: 0 };
    let total = 0;
    let current = 0;
    const sel = view.state.selection.main;
    try {
        const cursor = q.getCursor(view.state) as {
            next(): IteratorResult<{ from: number; to: number }>;
        };
        for (;;) {
            const r = cursor.next();
            if (r.done) break;
            total++;
            if (current === 0 && r.value.from <= sel.from && sel.from <= r.value.to) {
                current = total;
            }
            // A document with tens of thousands of hits reports "1000+" rather
            // than walking the whole thing on every keystroke.
            if (total >= MAX_COUNTED_MATCHES) break;
        }
    } catch {
        return { total: 0, current: 0 };
    }
    if (current === 0 && total > 0) current = 1;
    return { total, current };
}
