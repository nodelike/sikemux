import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { EditorState, Prec, type Text } from "@codemirror/state";
import { EditorView, keymap, type ViewUpdate } from "@codemirror/view";
import { copyLineDown, copyLineUp, indentWithTab } from "@codemirror/commands";
import { search } from "@codemirror/search";
import { basicSetup } from "codemirror";
import { auraExtensions, editorThemeOnlyExtensions, isLargeDoc, isSshConfigPath, languageFor, type EditorLanguageHint } from "../editor/codemirror";
import { isImagePath } from "../editor/media";
import { gitDiffGutter } from "../editor/gitGutter";
import { gitInlineBlame } from "../editor/gitBlame";
import { lspNav, setLspContext } from "../editor/lspNav";
import { lspHoverLink, setHoverLinkContext } from "../editor/lspHoverLink";
import { lspPeek } from "../editor/lspPeek";
import { fsapi, type FileBlob } from "../api/fs";
import type { LspTextChange } from "../api/lsp";
import { subscribe } from "../state/bus";
import * as cmd from "../state/commands";
import { invalidate } from "../state/resources";
import { useStore } from "../state/store";
import { errMessage, notify, reportError, swallow } from "../state/toast";
import { refreshViewTheme, registerView } from "../themes/bus";
import { useLspBridge } from "../hooks/useLspBridge";
import { useNavHistory, type NavEntry } from "../hooks/useNavHistory";
import { useGitBaseline } from "../hooks/useGitBaseline";
import { useGitBlame } from "../hooks/useGitBlame";
import type { CliPendingEditorOpen } from "../state/types";
import { FileTree, type CtxItem } from "./FileTree";
import { IconClose, IconFile } from "./Icons";
import { FileIcon } from "./FileIcon";
import { TabBar } from "./TabBar";
import { EditorFindBar } from "./EditorFindBar";
import { basename, isPathWithin, relativePath as pathRelative } from "../lib/paths";
import { FILE_MANAGER_NAME, PRIMARY_SHORTCUT } from "../lib/platform";

const DEFAULT_VIEW = { openTabs: [], activePath: null, treeWidth: 210 };
const EMPTY_CLI_OPENS: CliPendingEditorOpen[] = [];

function readSelection(view: EditorView): string | null {
    const sel = view.state.selection.main;
    if (sel.empty) return null;
    const raw = view.state.sliceDoc(sel.from, sel.to);
    const trimmed = raw
        .split(/\r?\n/)
        .find((l) => l.trim().length > 0)
        ?.trim();
    return trimmed && trimmed.length > 0 ? trimmed : null;
}

function scrollToLine(view: EditorView, line: number, character: number) {
    const lineCount = view.state.doc.lines;
    const ln = Math.max(1, Math.min(line + 1, lineCount));
    const lineObj = view.state.doc.line(ln);
    const pos = Math.min(lineObj.from + Math.max(0, character), lineObj.to);
    view.dispatch({
        selection: { anchor: pos },
        effects: EditorView.scrollIntoView(pos, { y: "center" }),
    });
    view.focus();
}

function lspPos(doc: Text, pos: number) {
    const line = doc.lineAt(pos);
    return { line: line.number - 1, character: pos - line.from };
}

function lspChangesFromUpdate(update: ViewUpdate): LspTextChange[] | null {
    const out: LspTextChange[] = [];
    let count = 0;
    update.changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
        count += 1;
        if (count > 1) return;
        out.push({
            range: { start: lspPos(update.startState.doc, fromA), end: lspPos(update.startState.doc, toA) },
            rangeLength: toA - fromA,
            text: inserted.toString(),
        });
    });
    // Multiple independent ranges in one CodeMirror transaction are relative to
    // the same start document; fall back to full sync rather than risk applying
    // shifted LSP ranges in the wrong order.
    return count === 1 ? out : null;
}

interface ImageState {
    path: string;
    loading: boolean;
    blob?: FileBlob;
    error?: string;
}

function formatBytes(n: number): string {
    if (n < 1024) return `${n} B`;
    const units = ["KB", "MB", "GB"];
    let v = n / 1024;
    let i = 0;
    while (v >= 1024 && i < units.length - 1) {
        v /= 1024;
        i += 1;
    }
    return `${v >= 10 ? v.toFixed(1) : v.toFixed(2)} ${units[i]}`;
}

function ImageViewer({ image, onReload }: { image: ImageState; onReload: (path: string) => void }) {
    const [dims, setDims] = useState<{ w: number; h: number } | null>(null);
    const [zoom, setZoom] = useState<"fit" | number>("fit");

    useEffect(() => {
        setDims(null);
        setZoom("fit");
    }, [image.path, image.blob?.data]);

    const src = image.blob ? `data:${image.blob.mime};base64,${image.blob.data}` : "";
    const zoomLabel = zoom === "fit" ? "fit" : `${Math.round(zoom * 100)}%`;

    return (
        <div className="ed-image-viewer">
            <div className="ed-image-bar">
                <div className="ed-image-title" title={image.path}>
                    <FileIcon name={basename(image.path)} size={16} />
                    <span>{basename(image.path)}</span>
                </div>
                <div className="ed-image-meta">
                    {image.blob && <span>{formatBytes(image.blob.size)}</span>}
                    {dims && (
                        <span>
                            {dims.w}×{dims.h}
                        </span>
                    )}
                    {image.blob && <span>{image.blob.mime}</span>}
                    <span>{zoomLabel}</span>
                </div>
                <div className="ed-image-actions">
                    <button type="button" onClick={() => setZoom("fit")} disabled={zoom === "fit"} title="Fit image to editor">
                        fit
                    </button>
                    <button type="button" onClick={() => setZoom(1)} disabled={zoom === 1} title="Actual size">
                        100%
                    </button>
                    <button type="button" onClick={() => setZoom((z) => (z === "fit" ? 1.25 : Math.min(z * 1.25, 8)))} title="Zoom in">
                        +
                    </button>
                    <button type="button" onClick={() => setZoom((z) => (z === "fit" ? 0.8 : Math.max(z / 1.25, 0.1)))} title="Zoom out">
                        −
                    </button>
                    <button type="button" onClick={() => onReload(image.path)} title="Reload image">
                        reload
                    </button>
                </div>
            </div>
            <div className="ed-image-stage">
                {image.loading && <div className="ed-image-message">loading image…</div>}
                {image.error && (
                    <div className="ed-image-message error">
                        <strong>couldn't open image</strong>
                        <span>{image.error}</span>
                    </div>
                )}
                {image.blob && !image.error && (
                    <img
                        src={src}
                        alt={basename(image.path)}
                        draggable={false}
                        onLoad={(e) => setDims({ w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight })}
                        style={
                            zoom === "fit"
                                ? undefined
                                : {
                                      width: `${Math.max(1, (dims?.w ?? 0) * zoom)}px`,
                                      height: "auto",
                                      maxWidth: "none",
                                      maxHeight: "none",
                                  }
                        }
                    />
                )}
            </div>
        </div>
    );
}

export function EditorPane({
    paneId,
    cwd,
    active,
    visible,
    showTree = true,
    onCloseWindow,
    languageHint,
}: {
    paneId: string;
    cwd: string;
    active: boolean;
    visible: boolean;
    showTree?: boolean;
    onCloseWindow?: () => void;
    languageHint?: EditorLanguageHint;
}) {
    const hostRef = useRef<HTMLDivElement>(null);
    const viewRef = useRef<EditorView | null>(null);
    const states = useRef<Map<string, EditorState>>(new Map());
    const currentRef = useRef<string | null>(null);
    const hydratedRef = useRef(false);
    const saveRef = useRef<() => boolean>(() => false);
    const openRequestRef = useRef(0);
    const processingCliOpenRef = useRef<string | null>(null);

    const [dirty, setDirty] = useState<ReadonlySet<string>>(() => new Set());
    const dirtyRef = useRef(dirty);
    dirtyRef.current = dirty;

    const savedRef = useRef<Map<string, string>>(new Map());
    const imagesRef = useRef<Map<string, FileBlob>>(new Map());
    const [activeImage, setActiveImage] = useState<ImageState | null>(null);

    const [findState, setFindState] = useState<{
        open: boolean;
        replaceOpen: boolean;
        seed: string | null;
        signal: number;
    }>({ open: false, replaceOpen: false, seed: null, signal: 0 });
    const openFindRef = useRef<(withReplace: boolean, seed: string | null) => void>(() => {});
    openFindRef.current = (withReplace, seed) => {
        setFindState((prev) => ({
            open: true,
            replaceOpen: withReplace || prev.replaceOpen,
            seed,
            signal: prev.signal + 1,
        }));
    };

    const view = useStore((s) => s.editorViews[paneId] ?? DEFAULT_VIEW);
    const pendingCliOpens = useStore((s) => s.pendingEditorOpens[paneId] ?? EMPTY_CLI_OPENS);
    const tabs = view.openTabs;
    const activePath = view.activePath;
    const treeWidth = view.treeWidth;

    const setTreeWidth = (w: number) => cmd.setEditorView(paneId, { treeWidth: w });

    useEffect(() => {
        cmd.setEditorDirtyPaths(paneId, [...dirty]);
    }, [paneId, dirty]);

    useEffect(() => {
        return () => cmd.setEditorDirtyPaths(paneId, []);
    }, [paneId]);

    const { openDoc, scheduleChange, saveDoc, closeDoc } = useLspBridge(cwd);

    const nav = useNavHistory({
        getView: () => viewRef.current,
        getCurrentPath: () => currentRef.current,
        scrollLiveTo: (l, c) => viewRef.current && scrollToLine(viewRef.current, l, c),
        openOther: (entry: NavEntry) => cmd.requestOpenFile(entry.path, entry.line, entry.character),
    });

    const bindLspContext = (view: EditorView, path: string | null) => {
        if (!path || !cwd || isImagePath(path)) {
            setLspContext(view, null);
            setHoverLinkContext(view, null);
            return;
        }
        setHoverLinkContext(view, { project: cwd, path });
        setLspContext(view, {
            project: cwd,
            path,
            navigate: (targetPath, line, character) => {
                nav.push({ path: targetPath, line, character });
            },
        });
    };

    const navBackRef = useRef(() => {});
    const navFwdRef = useRef(() => {});
    navBackRef.current = nav.back;
    navFwdRef.current = nav.forward;

    const save = useCallback((): boolean => {
        const path = currentRef.current;
        const view = viewRef.current;
        if (!path || !view || isImagePath(path)) return false;
        const text = view.state.doc.toString();
        void fsapi
            .writeFile(path, text)
            .then(() => {
                savedRef.current.set(path, text);
                const latest =
                    currentRef.current === path && viewRef.current ? viewRef.current.state.doc.toString() : states.current.get(path)?.doc.toString();
                if (latest !== text) return;
                setDirty((d) => {
                    if (!d.has(path)) return d;
                    const next = new Set(d);
                    next.delete(path);
                    return next;
                });
                if (cwd) {
                    invalidate((kind, args) => (kind.startsWith("git.") || kind === "files.list") && args[0] === cwd);
                    void saveDoc(path, text);
                }
                if (isSshConfigPath(path)) invalidate((kind) => kind === "ssh.hosts");
            })
            .catch(reportError("save"));
        return true;
    }, [cwd, saveDoc]);
    saveRef.current = save;

    const makeState = useCallback(
        (path: string, content: string) => {
            // Large files: skip the per-change (git diff) and per-mousemove (hover link)
            // extensions — they're the ones whose cost scales with the document.
            const heavy = isLargeDoc(content);
            const language = languageFor(path, languageHint);
            return EditorState.create({
                doc: content,
                extensions: [
                    basicSetup,
                    search({ top: true }),
                    heavy ? editorThemeOnlyExtensions() : auraExtensions,
                    ...(heavy && !languageHint ? [] : language),
                    ...(heavy ? [] : [gitDiffGutter(), gitInlineBlame(), lspHoverLink()]),
                    lspNav(),
                    lspPeek(),
                    keymap.of([indentWithTab]),
                    Prec.highest(
                        keymap.of([
                            { key: "Mod-Alt-ArrowUp", run: copyLineUp, preventDefault: true },
                            { key: "Mod-Alt-ArrowDown", run: copyLineDown, preventDefault: true },
                            { key: "Mod-s", preventDefault: true, run: () => saveRef.current() },
                            {
                                key: "Mod-[",
                                preventDefault: true,
                                run: () => {
                                    navBackRef.current();
                                    return true;
                                },
                            },
                            {
                                key: "Mod-]",
                                preventDefault: true,
                                run: () => {
                                    navFwdRef.current();
                                    return true;
                                },
                            },
                            {
                                key: "Mod-f",
                                preventDefault: true,
                                run: (view) => {
                                    openFindRef.current(false, readSelection(view));
                                    return true;
                                },
                            },
                            {
                                key: "Mod-h",
                                preventDefault: true,
                                run: (view) => {
                                    openFindRef.current(true, readSelection(view));
                                    return true;
                                },
                            },
                        ]),
                    ),
                    EditorView.updateListener.of((u) => {
                        if (!u.docChanged || !currentRef.current || isImagePath(currentRef.current)) return;
                        const p = currentRef.current;
                        const doc = u.state.doc;
                        const baseline = savedRef.current.get(p);
                        // Avoid serializing the whole doc on every keystroke: a length
                        // mismatch already proves it's dirty; only stringify when the
                        // lengths happen to match (e.g. an edit that reverts to saved).
                        const isDirty = baseline === undefined ? true : doc.length !== baseline.length ? true : doc.toString() !== baseline;
                        const has = dirtyRef.current.has(p);
                        if (isDirty && !has) {
                            setDirty((d) => new Set(d).add(p));
                        } else if (!isDirty && has) {
                            setDirty((d) => {
                                const next = new Set(d);
                                next.delete(p);
                                return next;
                            });
                        }
                        // Defer full serialization into the LSP debounce; normal
                        // typing goes over the bridge as a tiny incremental range.
                        scheduleChange(p, () => u.state.doc.toString(), lspChangesFromUpdate(u));
                    }),
                ],
            });
        },
        [languageHint, scheduleChange],
    );

    useEffect(() => {
        const view = new EditorView({ parent: hostRef.current!, state: makeState("", "") });
        viewRef.current = view;
        const unregister = registerView(view);
        return () => {
            unregister();
            view.destroy();
        };
    }, [makeState]);

    useEffect(() => {
        if (active && !activeImage) viewRef.current?.focus();
    }, [active, activePath, activeImage]);

    const showImage = useCallback((path: string, force = false) => {
        const cached = force ? undefined : imagesRef.current.get(path);
        if (cached) {
            setActiveImage({ path, loading: false, blob: cached });
            return;
        }
        setActiveImage({ path, loading: true });
        void fsapi
            .readFileBase64(path)
            .then((blob) => {
                imagesRef.current.set(path, blob);
                if (currentRef.current === path) setActiveImage({ path, loading: false, blob });
            })
            .catch((e) => {
                if (currentRef.current === path) setActiveImage({ path, loading: false, error: errMessage(e) });
            });
    }, []);

    const reloadImage = useCallback(
        (path: string) => {
            imagesRef.current.delete(path);
            showImage(path, true);
        },
        [showImage],
    );

    const switchTo = (path: string, fresh?: EditorState) => {
        const view = viewRef.current;
        if (!view) return;
        if (currentRef.current && !isImagePath(currentRef.current)) states.current.set(currentRef.current, view.state);

        if (isImagePath(path)) {
            currentRef.current = path;
            bindLspContext(view, null);
            showImage(path);
            cmd.setEditorView(paneId, { activePath: path });
            return;
        }

        const st = fresh ?? states.current.get(path);
        if (!st) return;
        setActiveImage(null);
        view.setState(st);
        refreshViewTheme(view);
        currentRef.current = path;
        bindLspContext(view, path);
        void openDoc(path, view.state.doc.toString());
        cmd.setEditorView(paneId, { activePath: path });
        view.focus();
    };

    const openPath = async (path: string) => {
        const request = ++openRequestRef.current;
        const liveTabs = useStore.getState().editorViews[paneId]?.openTabs ?? [];
        if (liveTabs.includes(path)) {
            if (isImagePath(path) || states.current.has(path)) {
                switchTo(path);
                return;
            }
        }
        if (isImagePath(path)) {
            const blob = await fsapi.readFileBase64(path);
            imagesRef.current.set(path, blob);
            cmd.openEditorTab(paneId, path);
            switchTo(path);
            return;
        }
        const content = await fsapi.readFile(path);
        const latest = request === openRequestRef.current;
        // Two rapid opens of the same path can resolve out of order. Do not
        // replace the state created by the newer request with the stale read.
        if (!latest && states.current.has(path)) {
            cmd.openEditorTab(paneId, path, false);
            return;
        }
        const st = makeState(path, content);
        states.current.set(path, st);
        savedRef.current.set(path, content);
        cmd.openEditorTab(paneId, path, latest);
        if (latest) switchTo(path, st);
    };

    useEffect(() => {
        const target = pendingCliOpens[0];
        if (!target) return;
        const key = `${target.requestId}\0${target.id}`;
        if (processingCliOpenRef.current) return;
        processingCliOpenRef.current = key;

        void (async () => {
            let error: string | null = null;
            try {
                await openPath(target.path);
                if (currentRef.current !== target.path) switchTo(target.path);
                if (target.line != null && viewRef.current && !isImagePath(target.path)) {
                    if (currentRef.current === target.path) {
                        scrollToLine(viewRef.current, target.line, target.column ?? 0);
                    }
                }
            } catch (cause) {
                error = errMessage(cause);
            }

            const stillQueued = (useStore.getState().pendingEditorOpens[paneId] ?? []).some(
                (item) => item.requestId === target.requestId && item.id === target.id,
            );
            if (!stillQueued && !error) {
                error = "The editor pane closed before Sikemux finished opening the file";
            }

            const result = {
                requestId: target.requestId,
                targetId: target.id,
                paneId: error ? null : paneId,
                path: target.path,
                error,
            };
            let delay = 50;
            for (let attempt = 0; attempt < 8; attempt += 1) {
                try {
                    await invoke("cli_open_result", { result });
                    break;
                } catch (cause) {
                    if (attempt === 7) swallow("CLI open acknowledgement")(cause);
                    else {
                        await new Promise((resolve) => setTimeout(resolve, delay));
                        delay = Math.min(delay * 2, 2_000);
                    }
                }
            }
            cmd.consumeCliEditorOpen(paneId, target.requestId, target.id);
            processingCliOpenRef.current = null;
        })();
        // openPath intentionally uses the latest editor refs. Queue changes are
        // the only trigger; the ref prevents overlapping reads during rerenders.
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [paneId, pendingCliOpens]);

    // The active tab was changed from outside this pane (⌥./⌥, cycling, or any
    // programmatic setEditorView): swap the live document to match. Tab clicks call
    // switchTo() directly, so they leave currentRef === activePath and no-op here.
    useEffect(() => {
        if (!hydratedRef.current || !activePath || currentRef.current === activePath) return;
        if (isImagePath(activePath) || states.current.has(activePath)) {
            switchTo(activePath);
            return;
        }
        let cancelled = false;
        (async () => {
            try {
                const content = await fsapi.readFile(activePath);
                if (cancelled) return;
                const st = makeState(activePath, content);
                states.current.set(activePath, st);
                savedRef.current.set(activePath, content);
                switchTo(activePath, st);
            } catch {}
        })();
        return () => {
            cancelled = true;
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [activePath]);

    useEffect(() => {
        if (!visible || hydratedRef.current) return;
        if (!viewRef.current) return;
        if (tabs.length === 0) return;
        let cancelled = false;
        (async () => {
            const want = activePath && tabs.includes(activePath) ? activePath : tabs[0];
            const load = async (path: string) => {
                if (isImagePath(path) || states.current.has(path)) return true;
                try {
                    const content = await fsapi.readFile(path);
                    if (cancelled) return false;
                    const st = makeState(path, content);
                    states.current.set(path, st);
                    savedRef.current.set(path, content);
                    return true;
                } catch {
                    cmd.setEditorView(paneId, {
                        openTabs: useStore.getState().editorViews[paneId]?.openTabs.filter((t) => t !== path) ?? [],
                    });
                    return false;
                }
            };

            if (want && (await load(want)) && !cancelled) {
                switchTo(want);
                hydratedRef.current = true;
            }

            // Warm the rest after the active tab is usable. This avoids blocking
            // first paint/focus on a pile of persisted tabs or one huge file.
            for (const path of tabs) {
                if (cancelled) return;
                if (path === want) continue;
                await load(path);
            }
            if (!cancelled) hydratedRef.current = true;
        })();
        return () => {
            cancelled = true;
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [visible]);

    useEffect(() => {
        if (!visible || !cwd) return;
        let cancelled = false;
        (async () => {
            const tabsNow = useStore.getState().editorViews[paneId]?.openTabs ?? [];
            for (const path of tabsNow) {
                if (cancelled || dirtyRef.current.has(path)) continue;
                if (isImagePath(path)) {
                    imagesRef.current.delete(path);
                    if (currentRef.current === path) showImage(path, true);
                    continue;
                }
                let fresh: string;
                try {
                    fresh = await fsapi.readFile(path);
                } catch {
                    continue;
                }
                if (cancelled) return;
                const isActive = currentRef.current === path;
                const view = viewRef.current;
                if (isActive && view) {
                    const doc = view.state.doc;
                    if (doc.length === fresh.length && doc.toString() === fresh) continue;
                    savedRef.current.set(path, fresh);
                    const head = Math.min(view.state.selection.main.head, fresh.length);
                    view.dispatch({
                        changes: { from: 0, to: view.state.doc.length, insert: fresh },
                        selection: { anchor: head },
                    });
                } else {
                    const cached = states.current.get(path);
                    if (cached && cached.doc.length === fresh.length && cached.doc.toString() === fresh) continue;
                    savedRef.current.set(path, fresh);
                    states.current.set(path, makeState(path, fresh));
                }
            }
        })();
        return () => {
            cancelled = true;
        };
    }, [visible, cwd, paneId, makeState, showImage]);

    useEffect(() => {
        if (!cwd || !visible) return;
        let timer: number | undefined;
        let running = false;
        let rerun = false;

        const refreshOpenTabs = async () => {
            if (running) {
                rerun = true;
                return;
            }
            running = true;
            try {
                const tabsNow = useStore.getState().editorViews[paneId]?.openTabs ?? [];
                for (const path of tabsNow) {
                    if (dirtyRef.current.has(path)) continue;
                    if (isImagePath(path)) {
                        imagesRef.current.delete(path);
                        if (currentRef.current === path) showImage(path, true);
                        continue;
                    }
                    let fresh: string;
                    try {
                        fresh = await fsapi.readFile(path);
                    } catch {
                        continue; // file was deleted / renamed — silently skip
                    }
                    const isActive = currentRef.current === path;
                    const view = viewRef.current;
                    if (isActive && view) {
                        const doc = view.state.doc;
                        if (doc.length === fresh.length && doc.toString() === fresh) continue;
                        // Do not collapse an in-progress drag selection because a
                        // watcher event arrived mid-gesture. Another debounced pass
                        // will apply the external update after the selection settles.
                        if (!view.state.selection.main.empty) {
                            rerun = true;
                            continue;
                        }
                        savedRef.current.set(path, fresh);
                        const head = Math.min(view.state.selection.main.head, fresh.length);
                        view.dispatch({
                            changes: { from: 0, to: view.state.doc.length, insert: fresh },
                            selection: { anchor: head },
                        });
                    } else {
                        const cached = states.current.get(path);
                        if (cached && cached.doc.length === fresh.length && cached.doc.toString() === fresh) continue;
                        savedRef.current.set(path, fresh);
                        states.current.set(path, makeState(path, fresh));
                    }
                }
            } finally {
                running = false;
                if (rerun) {
                    rerun = false;
                    timer = window.setTimeout(refreshOpenTabs, 300);
                }
            }
        };

        const unsubscribe = subscribe("fs-changed", (e) => {
            if (e.repo && e.repo !== cwd) return;
            if (timer) window.clearTimeout(timer);
            timer = window.setTimeout(refreshOpenTabs, 250);
        });
        return () => {
            unsubscribe();
            if (timer) window.clearTimeout(timer);
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [cwd, paneId, visible]);

    useEffect(() => {
        return subscribe("open-file", (e) => {
            // Project files open in their owning editor. LSP targets may live
            // in GOMODCACHE, rust stdlib, site-packages, etc.; route those to
            // the active editor instead of dropping them.
            const belongsHere = !!cwd && isPathWithin(e.path, cwd);
            const belongsToAProject = Object.values(useStore.getState().sessions).some((s) => s?.kind === "project" && isPathWithin(e.path, s.cwd));
            if (!belongsHere && (belongsToAProject || !active)) return;
            void (async () => {
                await openPath(e.path);
                if (e.line != null && viewRef.current && !isImagePath(e.path)) {
                    scrollToLine(viewRef.current, e.line, e.character ?? 0);
                }
            })().catch(reportError("open file"));
        });
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [cwd, active]);

    useEffect(() => {
        const view = viewRef.current;
        if (!view) return;
        if (!activePath || !cwd || isImagePath(activePath)) {
            setLspContext(view, null);
            setHoverLinkContext(view, null);
            return;
        }
        bindLspContext(view, activePath);
        return () => {
            setLspContext(view, null);
            setHoverLinkContext(view, null);
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [activePath, cwd]);

    useGitBaseline(() => viewRef.current, cwd, activePath);
    useGitBlame(() => viewRef.current, cwd, activePath);

    // Close an arbitrary set of tabs in one shot (used by the close button and the
    // tab context menu). Confirms once if any of them have unsaved changes, then
    // re-homes the active tab to the nearest survivor (VSCode-style).
    const closeTabs = (toClose: string[]) => {
        const closing = new Set(toClose.filter((p) => tabs.includes(p)));
        if (closing.size === 0) return;
        const dirtyClosing = [...closing].filter((p) => dirtyRef.current.has(p));
        if (dirtyClosing.length > 0) {
            const msg =
                dirtyClosing.length === 1
                    ? `Discard unsaved changes in ${basename(dirtyClosing[0])}?`
                    : `Discard unsaved changes in ${dirtyClosing.length} files?`;
            if (!window.confirm(msg)) {
                notify("info", "close cancelled — unsaved changes remain");
                return;
            }
        }
        for (const p of closing) {
            states.current.delete(p);
            imagesRef.current.delete(p);
            void closeDoc(p);
        }
        setDirty((d) => {
            let changed = false;
            const next = new Set(d);
            for (const p of closing) if (next.delete(p)) changed = true;
            return changed ? next : d;
        });
        const next = tabs.filter((t) => !closing.has(t));
        let nextActive = activePath;
        if (activePath && closing.has(activePath)) {
            const oldIdx = tabs.indexOf(activePath);
            let fallback: string | null = null;
            for (let i = oldIdx + 1; i < tabs.length && !fallback; i++) if (!closing.has(tabs[i])) fallback = tabs[i];
            for (let i = oldIdx - 1; i >= 0 && !fallback; i--) if (!closing.has(tabs[i])) fallback = tabs[i];
            nextActive = fallback;
            if (fallback) {
                switchTo(fallback);
            } else {
                currentRef.current = null;
                setActiveImage(null);
                viewRef.current?.setState(makeState("", ""));
            }
        }
        cmd.setEditorView(paneId, { openTabs: next, activePath: nextActive });
    };

    // ---- tab context menu ---------------------------------------------
    const relativePath = (p: string) => pathRelative(p, cwd) ?? basename(p);

    const copyText = (text: string, label: string) =>
        navigator.clipboard.writeText(text).then(() => notify("success", `copied ${label}`), reportError("copy"));

    const buildTabMenu = (path: string): CtxItem[] => {
        const idx = tabs.indexOf(path);
        const others = tabs.filter((t) => t !== path);
        const toLeft = tabs.slice(0, idx);
        const toRight = tabs.slice(idx + 1);
        const saved = tabs.filter((t) => !dirty.has(t));
        return [
            { label: "Close", hint: `${PRIMARY_SHORTCUT}W`, run: () => closeTabs([path]) },
            { label: "Close Others", disabled: others.length === 0, run: () => closeTabs(others) },
            { label: "Close to the Left", disabled: toLeft.length === 0, run: () => closeTabs(toLeft) },
            { label: "Close to the Right", disabled: toRight.length === 0, run: () => closeTabs(toRight) },
            { label: "Close Saved", disabled: saved.length === 0, run: () => closeTabs(saved) },
            { label: "Close All", run: () => closeTabs(tabs) },
            { sep: true },
            { label: "Copy Path", run: () => void copyText(path, "path") },
            { label: "Copy Relative Path", run: () => void copyText(relativePath(path), "relative path") },
            { sep: true },
            { label: `Reveal in ${FILE_MANAGER_NAME}`, run: () => void fsapi.revealInFinder(path).catch(reportError("reveal")) },
        ];
    };

    return (
        <div className="editor-pane">
            {showTree && (
                <FileTree
                    cwd={cwd}
                    activePath={activePath}
                    onOpenFile={(entry) => void openPath(entry.path).catch(reportError("open file"))}
                    width={treeWidth}
                    onResize={setTreeWidth}
                    active={visible}
                />
            )}
            <div className="ed-main">
                <TabBar
                    variant="editor"
                    tabs={tabs.map((path) => {
                        const name = basename(path);
                        return {
                            id: path,
                            label: name,
                            icon: <FileIcon name={name} size={18} />,
                            dirty: dirty.has(path),
                            active: activePath === path,
                            closable: onCloseWindow ? false : undefined,
                        };
                    })}
                    onSelect={(path) => switchTo(path)}
                    onClose={(path) => closeTabs([path])}
                    buildMenu={onCloseWindow ? undefined : buildTabMenu}
                    trailing={
                        onCloseWindow ? (
                            <button type="button" className="tabbar-window-close" title="Close SSH config" onClick={onCloseWindow}>
                                <IconClose size={12} />
                            </button>
                        ) : undefined
                    }
                />
                <div className={`ed-host${activeImage ? " image-mode" : ""}`} ref={hostRef}>
                    {!activeImage && (
                        <EditorFindBar
                            getView={() => viewRef.current}
                            open={findState.open}
                            replaceOpenOnMount={findState.replaceOpen}
                            seed={findState.seed}
                            signal={findState.signal}
                            onClose={() => setFindState((prev) => ({ ...prev, open: false }))}
                        />
                    )}
                    {activeImage && <ImageViewer image={activeImage} onReload={reloadImage} />}
                </div>
                {tabs.length === 0 && (
                    <div className="ed-empty">
                        <IconFile size={22} />
                        <p>select a file from the tree</p>
                        <p className="ed-empty-sub">Cmd-S saves · syntax-highlighted</p>
                    </div>
                )}
            </div>
        </div>
    );
}
