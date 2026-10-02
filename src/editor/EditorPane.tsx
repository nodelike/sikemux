import { FileTree } from "../rail/FileTree";
import { relocatedPath } from "../state/editorPaths";
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invokeCommand as invoke } from "../api/invoke";
import { Compartment, EditorState, Prec, type Text } from "@codemirror/state";
import { EditorView, keymap, type ViewUpdate } from "@codemirror/view";
import { copyLineDown, copyLineUp, indentWithTab } from "@codemirror/commands";
import { search } from "@codemirror/search";
import { basicSetup } from "codemirror";
import {
    auraExtensions,
    editorThemeOnlyExtensions,
    isLargeDoc,
    isSshConfigPath,
    LARGE_DOC_BYTES,
    languageCompartment,
    languageFor,
    loadLanguage,
    type EditorLanguageHint,
} from "./codemirror";
import { isPreviewPath } from "./viewers/fileKinds";
import type { ViewerState } from "./viewers/FileViewer";
import { Markdown, MARKDOWN_GFM, type MarkdownComponents } from "../markdown/Markdown";
import { gitDiffGutter } from "./gitGutter";
import { gitInlineBlame } from "./gitBlame";
import { DocumentIO } from "./documentIO";
import { lspNav, setLspContext } from "./lspNav";
import { lspHoverLink, setHoverLinkContext } from "./lspHoverLink";
import { lspPeek } from "./lspPeek";
import { fsapi, type FilePreview } from "../api/fs";
import type { LspTextChange } from "../api/lsp";
import { subscribe } from "../state/bus";
import * as cmd from "../state/commands";
import { invalidate } from "../state/resources";
import { useStore } from "../state/store";
import { collectPanes } from "../state/layout";
import { errCategory, errMessage, notify, reportError, swallow } from "../state/toast";
import { confirmDialog } from "../state/dialog";
import { refreshViewTheme, registerView } from "../themes/bus";
import { useLspBridge } from "../hooks/useLspBridge";
import { useNavHistory, type NavEntry } from "../hooks/useNavHistory";
import { useGitBaseline } from "../hooks/useGitBaseline";
import { useGitBlame } from "../hooks/useGitBlame";
import { refreshBlame } from "./gitBlame";
import type { CliPendingEditorOpen, DeskReveal } from "../state/types";
import { IconClose, IconEditor, IconEye, IconFile } from "../ui/Icons";
import { FileIcon } from "../ui/FileIcon";
import { TabBar } from "../workspace/TabBar";
import { EditorFindBar } from "./EditorFindBar";
import { EditorInsights } from "./EditorInsights";
import { PaneField } from "../ui/ShaderField";
import { basename, dirname, isPathWithin, joinPath, normalizePath } from "../lib/paths";
import { localPath } from "../chat/imagePreview";
import { safeWebUrl } from "../terminal/interactions";
import { keybindingLabelForAction } from "../commands/keybindings";

const FileViewer = lazy(() => import("./viewers/FileViewer"));

const DEFAULT_VIEW = { openTabs: [], activePath: null };
const EMPTY_CLI_OPENS: CliPendingEditorOpen[] = [];

function isMarkdownPath(path: string | null): path is string {
    return !!path && /\.(?:md|markdown)$/i.test(path);
}

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

interface Previewed {
    preview: FilePreview;
    revision: number;
}

function sameFile(a: FilePreview, b: FilePreview) {
    return a.size === b.size && a.modified === b.modified && a.mime === b.mime;
}

function markdownLinkFile(href: string, documentPath: string): string | null {
    if (href.startsWith("#")) return null;
    try {
        return localPath(new URL(href, `file://${normalizePath(dirname(documentPath)).split("/").map(encodeURIComponent).join("/")}/`).href);
    } catch {
        return null;
    }
}

function MarkdownPreview({ source, path, onOpenFile }: { source: string; path: string; onOpenFile: (path: string) => void }) {
    const components = useMemo<MarkdownComponents>(
        () => ({
            link: ({ href, children }) => (
                <a
                    href={href}
                    onClick={(event) => {
                        event.preventDefault();
                        if (!href) return;
                        const webUrl = safeWebUrl(href);
                        if (webUrl) {
                            void invoke("open_url", { url: webUrl, app: null, shortcut: null }).catch(swallow("open markdown link"));
                            return;
                        }
                        const file = markdownLinkFile(href, path);
                        if (file) onOpenFile(file);
                    }}>
                    {children}
                </a>
            ),
        }),
        [path, onOpenFile],
    );
    return (
        <div className="ed-markdown-preview">
            <article className="ed-markdown-body">
                <Markdown text={source} options={MARKDOWN_GFM} components={components} />
            </article>
        </div>
    );
}

export function EditorPane({
    paneId,
    cwd,
    active,
    visible,
    showInsights = true,
    onCloseWindow,
    languageHint,
    bare = false,
    reveal = null,
    onRevealed,
}: {
    paneId: string;
    cwd: string;
    active: boolean;
    visible: boolean;
    showInsights?: boolean;
    onCloseWindow?: () => void;
    languageHint?: EditorLanguageHint;
    /** An editor on an agent's desk: no file tree, and only the files the desk hands it. */
    bare?: boolean;
    reveal?: DeskReveal | null;
    onRevealed?: (seq: number) => void;
}) {
    const hostRef = useRef<HTMLDivElement>(null);
    const viewRef = useRef<EditorView | null>(null);
    const editableCompartment = useRef(new Compartment()).current;
    const states = useRef<Map<string, EditorState>>(new Map());
    const currentRef = useRef<string | null>(null);
    const hydratedRef = useRef(false);
    const saveRef = useRef<() => boolean>(() => false);
    const openRequestRef = useRef(0);
    const processingCliOpenRef = useRef<string | null>(null);
    const documentIORef = useRef(new DocumentIO());
    const saveSequenceRef = useRef<Map<string, number>>(new Map());
    const conflictedRef = useRef<Set<string>>(new Set());
    const showConflictRef = useRef<(path: string, detail: string) => void>(() => {});
    const reloadFromDiskRef = useRef<(path: string, announce?: boolean) => Promise<void>>(async () => {});

    const [treeWidth, setTreeWidth] = useState(240);
    const [dirty, setDirty] = useState<ReadonlySet<string>>(() => new Set());
    const dirtyRef = useRef(dirty);
    dirtyRef.current = dirty;

    const savedRef = useRef<Map<string, string>>(new Map());
    const previewsRef = useRef<Map<string, Previewed>>(new Map());
    const previewRevisionRef = useRef(0);
    const closeTabsRef = useRef<(paths: string[]) => void>(() => {});
    const [viewer, setViewer] = useState<ViewerState | null>(null);
    const [markdownPreview, setMarkdownPreview] = useState<{ path: string; content: string } | null>(null);

    const cacheState = (path: string, state: EditorState) => {
        states.current.delete(path);
        states.current.set(path, state);
        while (states.current.size > 16) {
            const candidate = [...states.current.keys()].find(
                (cachedPath) => cachedPath !== path && cachedPath !== currentRef.current && !dirtyRef.current.has(cachedPath),
            );
            if (!candidate) break;
            states.current.delete(candidate);
            savedRef.current.delete(candidate);
            documentIORef.current.forget(candidate);
        }
    };

    const showsViewer = (path: string) => previewsRef.current.has(path) || isPreviewPath(path);

    const [findState, setFindState] = useState<{
        open: boolean;
        replaceOpen: boolean;
        seed: string | null;
        signal: number;
    }>({ open: false, replaceOpen: false, seed: null, signal: 0 });
    const closeFind = useCallback(() => setFindState((prev) => ({ ...prev, open: false })), []);
    const getEditorView = useCallback(() => viewRef.current, []);
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
    const keybindingOverrides = useStore((s) => s.keybindingOverrides);
    const paneShader = useStore((s) => s.paneShader);
    const filePaletteHint = keybindingLabelForAction(keybindingOverrides, "palette.files");
    const pendingCliOpens = useStore((s) => s.pendingEditorOpens[paneId] ?? EMPTY_CLI_OPENS);
    const tabs = view.openTabs;
    const activePath = view.activePath;
    const previewingMarkdown = markdownPreview?.path === activePath;

    useEffect(() => {
        cmd.setEditorDirtyPaths(paneId, [...dirty]);
    }, [paneId, dirty]);

    useEffect(() => {
        if (view.preview && dirty.has(view.preview)) cmd.keepEditorTab(paneId, view.preview);
    }, [paneId, dirty, view.preview]);

    useEffect(() => {
        return () => cmd.setEditorDirtyPaths(paneId, []);
    }, [paneId]);

    const { openDoc, scheduleChange, saveDoc, closeDoc, diagnostics } = useLspBridge(cwd);

    const nav = useNavHistory({
        project: cwd,
        getView: () => viewRef.current,
        getCurrentPath: () => currentRef.current,
        scrollLiveTo: (l, c) => viewRef.current && scrollToLine(viewRef.current, l, c),
        openOther: (entry: NavEntry) => cmd.requestOpenFile(entry.path, entry.line, entry.character),
    });

    const bindLspContext = (view: EditorView, path: string | null) => {
        if (!path || !cwd || showsViewer(path)) {
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
        if (!path || !view || showsViewer(path)) return false;
        if (!dirtyRef.current.has(path)) {
            void reloadFromDiskRef.current(path, false).catch(swallow("refresh clean editor"));
            return true;
        }
        const text = view.state.doc.toString();
        const sequence = (saveSequenceRef.current.get(path) ?? 0) + 1;
        saveSequenceRef.current.set(path, sequence);
        void documentIORef.current
            .save(path, text)
            .then(() => {
                savedRef.current.set(path, text);
                conflictedRef.current.delete(path);
                const latest =
                    currentRef.current === path && viewRef.current ? viewRef.current.state.doc.toString() : states.current.get(path)?.doc.toString();
                if (latest !== text || saveSequenceRef.current.get(path) !== sequence) return;
                setDirty((d) => {
                    if (!d.has(path)) return d;
                    const next = new Set(d);
                    next.delete(path);
                    return next;
                });
                if (currentRef.current === path) refreshBlame(viewRef.current);
                if (cwd) {
                    // The repo watcher invalidates the git resources on its own
                    // shortly after the write; doing it here as well makes the
                    // backend walk the repository twice for one save.
                    invalidate((kind, args) => kind === "files.list" && args[0] === cwd);
                    void saveDoc(path, text);
                }
                if (isSshConfigPath(path)) invalidate((kind) => kind === "ssh.hosts");
            })
            .catch((error: unknown) => {
                if (errCategory(error) === "file-conflict") {
                    showConflictRef.current(path, errMessage(error));
                    return;
                }
                reportError("save")(error);
            });
        return true;
    }, [cwd, saveDoc]);
    saveRef.current = save;

    const makeState = useCallback(
        (path: string, content: string) => {
            // Large files: skip the per-change (git diff) and per-mousemove (hover link)
            // extensions — they're the ones whose cost scales with the document.
            const heavy = isLargeDoc(content);
            const wantsLanguage = !heavy || !!languageHint;
            if (wantsLanguage) void loadLanguage(path, languageHint).catch(swallow("editor language"));
            return EditorState.create({
                doc: content,
                extensions: [
                    basicSetup,
                    editableCompartment.of(EditorView.editable.of(true)),
                    search({ top: true }),
                    heavy ? editorThemeOnlyExtensions() : auraExtensions,
                    languageCompartment.of(wantsLanguage ? languageFor(path, languageHint) : []),
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
                        if (!u.docChanged || !currentRef.current || showsViewer(currentRef.current)) return;
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
        [editableCompartment, languageHint, scheduleChange],
    );

    reloadFromDiskRef.current = async (path: string, announce = true) => {
        const snapshot = await documentIORef.current.read(path);
        saveSequenceRef.current.set(path, (saveSequenceRef.current.get(path) ?? 0) + 1);
        savedRef.current.set(path, snapshot.content);
        conflictedRef.current.delete(path);
        const activeView = currentRef.current === path ? viewRef.current : null;
        if (activeView) {
            const head = Math.min(activeView.state.selection.main.head, snapshot.content.length);
            activeView.dispatch({
                changes: { from: 0, to: activeView.state.doc.length, insert: snapshot.content },
                selection: { anchor: head },
            });
        } else {
            cacheState(path, makeState(path, snapshot.content));
        }
        setMarkdownPreview((preview) => (preview?.path === path ? { path, content: snapshot.content } : preview));
        setDirty((dirtyPaths) => {
            if (!dirtyPaths.has(path)) return dirtyPaths;
            const next = new Set(dirtyPaths);
            next.delete(path);
            return next;
        });
        if (announce) notify("success", `reloaded ${basename(path)} from disk`);
    };

    showConflictRef.current = (path, detail) => {
        if (!dirtyRef.current.has(path)) {
            void reloadFromDiskRef.current(path, false).catch(swallow("refresh clean editor"));
            return;
        }
        conflictedRef.current.add(path);
        notify("error", `${basename(path)} has an external change. Your editor buffer was preserved. ${detail}`, {
            timeoutMs: null,
            action: {
                label: "Reload disk",
                dismissOnClick: true,
                run: () => reloadFromDiskRef.current(path).catch(reportError("reload file")),
            },
        });
    };

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
        const editorView = viewRef.current;
        if (!editorView) return;
        editorView.dispatch({ effects: editableCompartment.reconfigure(EditorView.editable.of(!previewingMarkdown)) });
        if (previewingMarkdown) editorView.contentDOM.blur();
    }, [editableCompartment, previewingMarkdown]);

    useEffect(() => {
        if (active && !viewer && !previewingMarkdown) viewRef.current?.focus();
    }, [active, activePath, viewer, previewingMarkdown]);

    useEffect(() => {
        setMarkdownPreview(null);
    }, [activePath]);

    /** Asks the backend about the file again; the revision only moves when the file did. */
    const loadPreview = async (path: string, force = false): Promise<Previewed> => {
        const preview = await fsapi.previewFile(path);
        const held = previewsRef.current.get(path);
        const previewed = held && !force && sameFile(held.preview, preview) ? held : { preview, revision: ++previewRevisionRef.current };
        previewsRef.current.set(path, previewed);
        return previewed;
    };

    /** The file as editor text, or null when it is not text and opens in a viewer instead. */
    const readDocument = async (path: string) => {
        try {
            return await documentIORef.current.read(path);
        } catch (error) {
            if (errCategory(error) !== "not-text") throw error;
            await loadPreview(path);
            return null;
        }
    };

    /* Shows what is already known at once, then checks the disk; "reload" loads the file again even if it looks unchanged. */
    const showViewerRef = useRef<(path: string, mode?: "refresh" | "reload") => void>(() => {});
    showViewerRef.current = (path, mode = "refresh") => {
        const held = previewsRef.current.get(path);
        setViewer(held ? { path, ...held } : { path, revision: 0 });
        loadPreview(path, mode === "reload")
            .then((previewed) => {
                if (currentRef.current === path) setViewer({ path, ...previewed });
            })
            .catch((error: unknown) => {
                if (currentRef.current === path) setViewer({ path, revision: 0, error: errMessage(error) });
            });
    };
    const reloadViewer = useCallback((path: string) => showViewerRef.current(path, "reload"), []);

    const switchTo = (path: string, fresh?: EditorState) => {
        const view = viewRef.current;
        if (!view) return;
        if (currentRef.current && !showsViewer(currentRef.current)) cacheState(currentRef.current, view.state);

        if (showsViewer(path)) {
            currentRef.current = path;
            bindLspContext(view, null);
            showViewerRef.current(path);
            cmd.setEditorView(paneId, { activePath: path });
            return;
        }

        const st = fresh ?? states.current.get(path);
        if (!st) return;
        setViewer(null);
        view.setState(st);
        refreshViewTheme(view);
        currentRef.current = path;
        bindLspContext(view, path);
        void openDoc(path, view.state.doc.toString());
        cmd.setEditorView(paneId, { activePath: path });
        view.focus();
    };

    const openPathRef = useRef<(path: string, preview?: boolean) => Promise<void>>(async () => {});
    const previewTreeFile = useCallback((entry: { path: string }) => {
        void openPathRef.current(entry.path, true).catch(reportError("open file"));
    }, []);
    const keepTreeFile = useCallback((entry: { path: string }) => {
        void openPathRef.current(entry.path).catch(reportError("open file"));
    }, []);

    const openLinkedFile = useCallback((path: string) => {
        void openPathRef.current(path).catch(reportError("open linked file"));
    }, []);

    const openTab = (path: string, activate: boolean, preview: boolean) => {
        const replaced = cmd.openEditorTab(paneId, path, activate, preview);
        if (replaced && replaced !== path) forgetDocs([replaced]);
    };

    const openPath = async (path: string, preview = false) => {
        const request = ++openRequestRef.current;
        const liveTabs = useStore.getState().editorViews[paneId]?.openTabs ?? [];
        if (liveTabs.includes(path)) {
            if (!preview) cmd.keepEditorTab(paneId, path);
            if (showsViewer(path) || states.current.has(path)) {
                switchTo(path);
                return;
            }
        }
        if (isPreviewPath(path)) await loadPreview(path);
        const snapshot = isPreviewPath(path) ? null : await readDocument(path);
        if (!snapshot) {
            openTab(path, true, preview);
            switchTo(path);
            return;
        }
        const content = snapshot.content;
        const latest = request === openRequestRef.current;
        // Two rapid opens of the same path can resolve out of order. Do not
        // replace the state created by the newer request with the stale read.
        if (!latest && states.current.has(path)) {
            openTab(path, false, preview);
            return;
        }
        const st = makeState(path, content);
        cacheState(path, st);
        savedRef.current.set(path, content);
        openTab(path, latest, preview);
        if (latest) switchTo(path, st);
    };
    openPathRef.current = openPath;

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
                if (target.line != null && viewRef.current && !showsViewer(target.path)) {
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
        // eslint-disable-next-line react-hooks/exhaustive-deps -- only a change to the queue should start an open
    }, [paneId, pendingCliOpens]);

    // The active tab was changed from outside this pane (⌥./⌥, cycling, or any
    // programmatic setEditorView): swap the live document to match. Tab clicks call
    // switchTo() directly, so they leave currentRef === activePath and no-op here.
    useEffect(() => {
        if (!hydratedRef.current || !activePath || currentRef.current === activePath) return;
        if (showsViewer(activePath) || states.current.has(activePath)) {
            switchTo(activePath);
            return;
        }
        let cancelled = false;
        (async () => {
            try {
                const snapshot = await readDocument(activePath);
                if (cancelled) return;
                if (!snapshot) {
                    switchTo(activePath);
                    return;
                }
                const content = snapshot.content;
                const st = makeState(activePath, content);
                cacheState(activePath, st);
                savedRef.current.set(activePath, content);
                switchTo(activePath, st);
            } catch {}
        })();
        return () => {
            cancelled = true;
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps -- makeState and switchTo are rebuilt every render; only an outside tab change runs this
    }, [activePath]);

    useEffect(() => {
        if (!visible || hydratedRef.current) return;
        if (!viewRef.current) return;
        if (tabs.length === 0) return;
        let cancelled = false;
        (async () => {
            const want = activePath && tabs.includes(activePath) ? activePath : tabs[0];
            const load = async (path: string) => {
                if (showsViewer(path) || states.current.has(path)) return true;
                try {
                    const snapshot = await readDocument(path);
                    if (cancelled) return false;
                    if (!snapshot) return true;
                    const content = snapshot.content;
                    const st = makeState(path, content);
                    cacheState(path, st);
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

            if (!cancelled) hydratedRef.current = true;
        })();
        return () => {
            cancelled = true;
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps -- saved tabs are restored once, when the pane first becomes visible
    }, [visible]);

    useEffect(() => {
        if (!visible || !cwd || !hydratedRef.current) return;
        let cancelled = false;
        (async () => {
            const path = currentRef.current;
            if (!path || dirtyRef.current.has(path)) return;
            if (showsViewer(path)) {
                showViewerRef.current(path);
                return;
            }
            try {
                const snapshot = await documentIORef.current.read(path);
                if (cancelled || currentRef.current !== path) return;
                const editor = viewRef.current;
                if (!editor || (editor.state.doc.length === snapshot.content.length && editor.state.doc.toString() === snapshot.content)) return;
                savedRef.current.set(path, snapshot.content);
                const head = Math.min(editor.state.selection.main.head, snapshot.content.length);
                editor.dispatch({
                    changes: { from: 0, to: editor.state.doc.length, insert: snapshot.content },
                    selection: { anchor: head },
                });
            } catch (error) {
                swallow("refresh clean editor")(error);
            }
        })();
        return () => {
            cancelled = true;
        };
    }, [visible, cwd, paneId, makeState]);

    useEffect(() => {
        if (!cwd || !visible) return;
        let timer: number | undefined;
        let running = false;
        let rerun = false;
        let cancelled = false;
        let pendingPaths: Set<string> | null = new Set();

        const refreshOpenTabs = async () => {
            if (running) {
                rerun = true;
                return;
            }
            running = true;
            const changed = pendingPaths ? [...pendingPaths] : null;
            pendingPaths = new Set();
            try {
                const tabsNow = useStore.getState().editorViews[paneId]?.openTabs ?? [];
                for (const path of tabsNow) {
                    if (cancelled) return;
                    if (changed && !changed.some((entry) => isPathWithin(path, entry))) continue;
                    if (showsViewer(path)) {
                        if (currentRef.current === path) showViewerRef.current(path);
                        continue;
                    }
                    if (dirtyRef.current.has(path)) {
                        try {
                            const snapshot = await documentIORef.current.peek(path);
                            if (cancelled) return;
                            if (documentIORef.current.changedSinceObserved(path, snapshot) && !conflictedRef.current.has(path)) {
                                showConflictRef.current(path, "The disk version changed while this editor had unsaved work.");
                            }
                        } catch (error) {
                            if (!conflictedRef.current.has(path)) showConflictRef.current(path, errMessage(error));
                        }
                        continue;
                    }
                    let fresh: string;
                    const known = documentIORef.current.version(path);
                    try {
                        const snapshot = await documentIORef.current.read(path);
                        // A path-less watcher event asks about every open tab; most
                        // of them are still the version this pane already holds.
                        if (known !== undefined && snapshot.version === known) continue;
                        fresh = snapshot.content;
                    } catch (error) {
                        swallow("refresh clean editor")(error);
                        continue;
                    }
                    if (cancelled || dirtyRef.current.has(path)) continue;
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
                            pendingPaths?.add(path);
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
                        cacheState(path, makeState(path, fresh));
                    }
                }
            } finally {
                running = false;
                if (rerun && !cancelled) {
                    rerun = false;
                    timer = window.setTimeout(refreshOpenTabs, 300);
                }
            }
        };

        const unsubscribe = subscribe("fs-changed", (e) => {
            if (e.repo && e.repo !== cwd) return;
            if (!e.paths) pendingPaths = null;
            else for (const path of e.paths) pendingPaths?.add(joinPath(cwd, path));
            if (timer) window.clearTimeout(timer);
            timer = window.setTimeout(refreshOpenTabs, 250);
        });
        return () => {
            cancelled = true;
            unsubscribe();
            if (timer) window.clearTimeout(timer);
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps -- a new language hint does not need a new file watcher
    }, [cwd, paneId, visible]);

    const pathRenamedRef = useRef<(event: { src: string; dest: string }) => void>(() => {});
    pathRenamedRef.current = ({ src, dest }) => {
        const paths = useStore.getState().editorViews[paneId]?.openTabs ?? [];
        for (const path of paths) {
            const next = relocatedPath(path, src, dest);
            if (next === path) continue;
            const view = currentRef.current === path ? viewRef.current : null;
            const state = view?.state ?? states.current.get(path);
            if (state) {
                cacheState(next, state);
                states.current.delete(path);
            }
            const saved = savedRef.current.get(path);
            if (saved !== undefined) {
                savedRef.current.set(next, saved);
                savedRef.current.delete(path);
            }
            const previewed = previewsRef.current.get(path);
            if (previewed) {
                previewsRef.current.set(next, previewed);
                previewsRef.current.delete(path);
            }
            documentIORef.current.relocate(path, next);
            conflictedRef.current.delete(path);
            saveSequenceRef.current.delete(path);
            void closeDoc(path);
            if (state) void openDoc(next, state.doc.toString());
            if (currentRef.current === path) {
                currentRef.current = next;
                if (view) bindLspContext(view, next);
                if (previewed) showViewerRef.current(next, "reload");
            }
        }
        setDirty((paths) => new Set([...paths].map((path) => relocatedPath(path, src, dest))));
        setMarkdownPreview((preview) => (preview ? { ...preview, path: relocatedPath(preview.path, src, dest) } : preview));
    };

    useEffect(() => subscribe("path-renamed", (event) => pathRenamedRef.current(event)), []);

    useEffect(() => {
        return subscribe("close-file", (e) => {
            if (e.paneId === paneId) closeTabsRef.current([e.path]);
        });
    }, [paneId]);

    useEffect(() => {
        if (!reveal) return;
        void (async () => {
            await openPath(reveal.path);
            hydratedRef.current = true;
            if (reveal.line != null && viewRef.current && !showsViewer(reveal.path))
                scrollToLine(viewRef.current, reveal.line, reveal.character ?? 0);
        })()
            .catch(reportError("open file"))
            .finally(() => onRevealed?.(reveal.seq));
        // eslint-disable-next-line react-hooks/exhaustive-deps -- each reveal runs once, keyed by its sequence number
    }, [reveal?.seq]);

    useEffect(() => {
        if (bare) return;
        return subscribe("open-file", (e) => {
            // A view split beside other work shows the one file it was given.
            const { windows, editorViews } = useStore.getState();
            const ownWindow = Object.values(windows).find((win) => collectPanes(win.root).some((pane) => pane.id === paneId));
            if ((ownWindow && ownWindow.role !== "files") || editorViews[paneId]?.single) return;
            // Project files open in their owning editor. LSP targets may live
            // in GOMODCACHE, rust stdlib, site-packages, etc.; route those to
            // the active editor instead of dropping them.
            const belongsHere = !!cwd && isPathWithin(e.path, cwd);
            const belongsToAProject = Object.values(useStore.getState().sessions).some((s) => s?.kind === "project" && isPathWithin(e.path, s.cwd));
            if (!belongsHere && (belongsToAProject || !active)) return;
            void (async () => {
                await openPathRef.current(e.path);
                if (e.line != null && viewRef.current && !showsViewer(e.path)) {
                    scrollToLine(viewRef.current, e.line, e.character ?? 0);
                }
            })().catch(reportError("open file"));
        });
    }, [bare, cwd, active, paneId]);

    useEffect(() => {
        const view = viewRef.current;
        if (!view) return;
        if (!activePath || !cwd || showsViewer(activePath)) {
            setLspContext(view, null);
            setHoverLinkContext(view, null);
            return;
        }
        bindLspContext(view, activePath);
        return () => {
            setLspContext(view, null);
            setHoverLinkContext(view, null);
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps -- bindLspContext is rebuilt every render but only reads cwd and the stable nav history
    }, [activePath, cwd]);

    // The grammar packs download per language, so a freshly opened document may
    // start plain and gain its highlighting a moment later.
    useEffect(() => {
        if (!activePath || showsViewer(activePath)) return;
        let cancelled = false;
        void loadLanguage(activePath, languageHint)
            .then((extensions) => {
                const view = viewRef.current;
                if (cancelled || extensions.length === 0 || !view || currentRef.current !== activePath) return;
                if (view.state.doc.length > LARGE_DOC_BYTES && !languageHint) return;
                if (languageCompartment.get(view.state) === extensions) return;
                view.dispatch({ effects: languageCompartment.reconfigure(extensions) });
            })
            .catch(swallow("editor language"));
        return () => {
            cancelled = true;
        };
    }, [activePath, languageHint]);

    useGitBaseline(() => viewRef.current, cwd, activePath);
    useGitBlame(() => viewRef.current, cwd, activePath);

    // Close an arbitrary set of tabs in one shot (used by the close button and the
    // tab context menu). Confirms once if any of them have unsaved changes, then
    // re-homes the active tab to the nearest survivor (VSCode-style).
    const closeTabs = (toClose: string[]) => {
        const closing = new Set(toClose.filter((p) => tabs.includes(p)));
        if (closing.size === 0) return;
        const dirtyClosing = [...closing].filter((p) => dirtyRef.current.has(p));
        if (dirtyClosing.length === 0) {
            closeTabsNow(closing);
            return;
        }
        void confirmDialog({
            title: "Discard unsaved changes?",
            body:
                dirtyClosing.length === 1
                    ? `Edits in ${basename(dirtyClosing[0])} will be lost.`
                    : `Edits in ${dirtyClosing.length} files will be lost.`,
            confirmLabel: "Discard",
            destructive: true,
        }).then((ok) => {
            if (ok) closeTabsNow(closing);
            else notify("info", "close cancelled — unsaved changes remain");
        });
    };
    closeTabsRef.current = closeTabs;

    const forgetDocs = (paths: Iterable<string>) => {
        const forgotten = new Set(paths);
        for (const p of forgotten) {
            states.current.delete(p);
            previewsRef.current.delete(p);
            savedRef.current.delete(p);
            saveSequenceRef.current.delete(p);
            conflictedRef.current.delete(p);
            documentIORef.current.forget(p);
            void closeDoc(p);
        }
        setDirty((d) => {
            let changed = false;
            const next = new Set(d);
            for (const p of forgotten) if (next.delete(p)) changed = true;
            return changed ? next : d;
        });
    };

    const closeTabsNow = (closing: Set<string>) => {
        forgetDocs(closing);
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
                setViewer(null);
                viewRef.current?.setState(makeState("", ""));
            }
        }
        cmd.setEditorView(paneId, {
            openTabs: next,
            activePath: nextActive,
            preview: view.preview && closing.has(view.preview) ? undefined : view.preview,
        });
    };

    const toggleMarkdownPreview = () => {
        if (!isMarkdownPath(activePath)) return;
        if (previewingMarkdown) {
            setMarkdownPreview(null);
            return;
        }
        const editorView = viewRef.current;
        if (!editorView || currentRef.current !== activePath) return;
        setFindState((state) => ({ ...state, open: false }));
        setMarkdownPreview({ path: activePath, content: editorView.state.doc.toString() });
    };

    return (
        <div className="editor-pane">
            {!onCloseWindow && !bare && (
                <FileTree
                    width={treeWidth}
                    onResize={setTreeWidth}
                    cwd={cwd}
                    activePath={activePath}
                    onOpenFile={previewTreeFile}
                    onKeepFile={keepTreeFile}
                    active={visible}
                />
            )}
            <div className="ed-main">
                {!bare && <PaneField enabled={paneShader && visible} />}
                {/* An ordinary editor's documents are tabs in the session
                    strip, so the only bar left here is the one an SSH config
                    window needs to close itself. */}
                {onCloseWindow ? (
                    <TabBar
                        variant="editor"
                        tabs={tabs.map((path) => {
                            const name = basename(path);
                            return {
                                id: path,
                                tabId: `editor-tab-${paneId}-${encodeURIComponent(path)}`,
                                panelId: `editor-content-${paneId}`,
                                label: name,
                                icon: <FileIcon name={name} size={18} />,
                                dirty: dirty.has(path),
                                active: activePath === path,
                                closable: false,
                            };
                        })}
                        onSelect={(path) => switchTo(path)}
                        trailing={
                            <button type="button" className="tabbar-window-close" title="Close SSH config" onClick={onCloseWindow}>
                                <IconClose size={12} />
                            </button>
                        }
                    />
                ) : (
                    isMarkdownPath(activePath) && (
                        <div className="ed-toolbar">
                            <button
                                type="button"
                                className="ed-markdown-toggle"
                                aria-label={previewingMarkdown ? `Show source for ${basename(activePath)}` : `Preview ${basename(activePath)}`}
                                aria-pressed={previewingMarkdown}
                                onClick={toggleMarkdownPreview}>
                                {previewingMarkdown ? <IconEditor size={13} /> : <IconEye size={13} />}
                                <span>{previewingMarkdown ? "Source" : "Preview"}</span>
                            </button>
                        </div>
                    )
                )}
                <div
                    id={`editor-content-${paneId}`}
                    role={onCloseWindow ? "tabpanel" : undefined}
                    aria-labelledby={onCloseWindow && activePath ? `editor-tab-${paneId}-${encodeURIComponent(activePath)}` : undefined}
                    className={`ed-host${viewer ? " viewer-mode" : ""}${previewingMarkdown ? " preview-mode" : ""}`}>
                    <div
                        className="ed-source-host"
                        hidden={!!viewer || previewingMarkdown}
                        aria-hidden={!!viewer || previewingMarkdown}
                        ref={hostRef}
                    />
                    {!viewer && !previewingMarkdown && (
                        <EditorFindBar
                            getView={getEditorView}
                            documentKey={activePath}
                            open={findState.open}
                            replaceOpenOnMount={findState.replaceOpen}
                            seed={findState.seed}
                            signal={findState.signal}
                            onClose={closeFind}
                        />
                    )}
                    {viewer && (
                        <Suspense>
                            <FileViewer viewer={viewer} visible={visible} onReload={reloadViewer} />
                        </Suspense>
                    )}
                    {previewingMarkdown && (
                        <MarkdownPreview source={markdownPreview.content} path={markdownPreview.path} onOpenFile={openLinkedFile} />
                    )}
                </div>
                {showInsights && cwd && (
                    <EditorInsights
                        project={cwd}
                        path={activePath}
                        controller={diagnostics}
                        visible={visible}
                        onNavigate={(path, line, character) => nav.push({ path, line, character })}
                        paneId={paneId}
                    />
                )}
                {tabs.length === 0 && !bare && (
                    <div className="ed-empty">
                        <IconFile size={22} />
                        <p>Open a file to get started</p>
                        <p className="ed-empty-sub">Browse the project tree or search by name.</p>
                        <button type="button" className="settings-btn primary" onClick={cmd.openFilePalette}>
                            Open file <kbd>{filePaletteHint}</kbd>
                        </button>
                    </div>
                )}
            </div>
        </div>
    );
}
