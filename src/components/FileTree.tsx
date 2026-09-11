import { renameEditorPath } from "../state/editorPaths";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent, ReactNode } from "react";
import { fsapi, type DirEntry } from "../api/fs";
import { type GitFile } from "../api/git";
import { subscribe } from "../state/bus";
import { useResourceEnabled } from "../state/resources";
import { gitStatusR } from "../state/resources.defs";
import { notify, reportError, swallow } from "../state/toast";
import { confirmDialog } from "../state/dialog";
import { registerFolderDrop } from "../state/dropRegistry";
import { IconChevron, IconFolder, IconPlus } from "./Icons";
import { FileIcon } from "./FileIcon";
import { Tooltip } from "./Tooltip";
import { gitFileDecoration } from "./git/gitFileStatus";
import { basename, dirname, isPathWithin, joinPath, normalizePath, relativePath as pathRelative } from "../lib/paths";
import { FILE_MANAGER_NAME } from "../lib/platform";

interface FileTreeProps {
    cwd: string;
    activePath: string | null;
    onOpenFile: (entry: DirEntry) => void;
    /** Omit both to let the tree fill its container instead of owning a width. */
    width?: number;
    onResize?: (w: number) => void;
    active: boolean;
    revealPath?: string | null;
}

interface NewEntryRequest {
    parent: string;
    kind: "file" | "folder";
}

interface MenuState {
    x: number;
    y: number;
    entry: DirEntry | null;
}

type VisibleTreeRow =
    { key: string; kind: "entry"; entry: DirEntry; depth: number } | { key: string; kind: "new"; depth: number; request: NewEntryRequest };

const TREE_ROW_HEIGHT = 23;
const TREE_VIRTUALIZE_AFTER = 150;

export interface CtxItem {
    label?: string;
    hint?: string;
    danger?: boolean;
    disabled?: boolean;
    sep?: boolean;
    run?: () => void;
}

function validEntryName(raw: string): string | null {
    const name = raw.trim();
    if (!name) return null;
    if (name === "." || name === ".." || name.includes("/") || name.includes("\\") || name.includes("\0")) return null;
    return name;
}

export function FileTree({ cwd, activePath, onOpenFile, width, onResize, active, revealPath }: FileTreeProps) {
    const resizable = width !== undefined && onResize !== undefined;
    const [dirs, setDirs] = useState<Record<string, DirEntry[]>>({});
    const [expanded, setExpanded] = useState<Set<string>>(new Set());
    const [selectedDir, setSelectedDir] = useState<string | null>(null);
    const [newRequest, setNewRequest] = useState<NewEntryRequest | null>(null);
    const [newName, setNewName] = useState("");
    const newInputRef = useRef<HTMLInputElement>(null);
    const [renaming, setRenaming] = useState<string | null>(null);
    const [renameName, setRenameName] = useState("");
    const renameInputRef = useRef<HTMLInputElement>(null);
    const [dragOver, setDragOver] = useState<string | null>(null);
    const [rootDragOver, setRootDragOver] = useState(false);
    const [draggingPath, setDraggingPath] = useState<string | null>(null);
    const [dragGhost, setDragGhost] = useState<{ name: string; x: number; y: number } | null>(null);
    const [focusedPath, setFocusedPath] = useState<string | null>(null);
    const [menu, setMenu] = useState<MenuState | null>(null);
    const rootScrollRef = useRef<HTMLDivElement>(null);
    const rowButtonRefs = useRef(new Map<string, HTMLButtonElement>());

    const expandedRef = useRef(expanded);
    expandedRef.current = expanded;

    const status = useResourceEnabled(active && !!cwd, gitStatusR, cwd || "");
    const gitMap = useMemo(() => {
        const m = new Map<string, GitFile>();
        if (cwd && status.data) {
            status.data.files.forEach((f) => m.set(joinPath(cwd, f.path), f));
        }
        return m;
    }, [cwd, status.data]);

    const visibleRows = useMemo(() => {
        const rows: VisibleTreeRow[] = [];
        const collect = (path: string, depth: number) => {
            for (const entry of dirs[path] ?? []) {
                rows.push({ key: entry.path, kind: "entry", entry, depth });
                if (entry.is_dir && expanded.has(entry.path)) collect(entry.path, depth + 1);
                if (newRequest?.parent === entry.path) {
                    rows.push({ key: `new:${entry.path}`, kind: "new", depth: depth + 1, request: newRequest });
                }
            }
        };
        collect(cwd, 0);
        if (newRequest?.parent === cwd) rows.push({ key: `new:${cwd}`, kind: "new", depth: 0, request: newRequest });
        return rows;
    }, [cwd, dirs, expanded, newRequest]);
    const entryRows = useMemo(
        () => visibleRows.filter((row): row is Extract<VisibleTreeRow, { kind: "entry" }> => row.kind === "entry"),
        [visibleRows],
    );
    const focusPath = focusedPath && entryRows.some((row) => row.entry.path === focusedPath) ? focusedPath : entryRows[0]?.entry.path;
    const virtualized = visibleRows.length > TREE_VIRTUALIZE_AFTER;
    const treeVirtualizer = useVirtualizer({
        count: visibleRows.length,
        getScrollElement: () => rootScrollRef.current,
        estimateSize: () => TREE_ROW_HEIGHT,
        getItemKey: (index) => visibleRows[index]?.key ?? index,
        overscan: 20,
        enabled: virtualized,
    });

    const loadDir = useCallback((path: string) => {
        return fsapi
            .readDir(path)
            .then((e) => {
                setDirs((d) => ({ ...d, [path]: e }));
            })
            .catch(swallow("readDir"));
    }, []);

    useEffect(() => {
        if (!cwd || !active) return;
        void loadDir(cwd);
    }, [cwd, active, loadDir]);

    useEffect(() => {
        if (!cwd || !active) return;
        const unsubscribe = subscribe("fs-changed", (e) => {
            if (e.repo && e.repo !== cwd) return;
            const changed = e.paths?.map((path) => joinPath(cwd, path));
            for (const path of [cwd, ...expandedRef.current]) {
                if (!changed || changed.some((entry) => dirname(entry) === path || isPathWithin(path, entry))) void loadDir(path);
            }
        });
        return unsubscribe;
    }, [cwd, active, loadDir]);

    useEffect(() => {
        const path = revealPath ?? activePath;
        if (!active || !path || !cwd) return;
        const rel = pathRelative(path, cwd);
        if (!rel) return;
        const parts = rel.split("/");
        if (parts.length < 2) return;
        const parents: string[] = [];
        let p = cwd;
        for (let i = 0; i < parts.length - 1; i++) {
            p = joinPath(p, parts[i]);
            parents.push(p);
        }
        setExpanded((s) => {
            const n = new Set(s);
            parents.forEach((x) => n.add(x));
            return n;
        });
        for (const par of parents) {
            if (!dirs[par]) void loadDir(par);
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [revealPath, activePath, cwd, active]);

    useEffect(() => {
        if (!newRequest) return;
        const rowIndex = visibleRows.findIndex((row) => row.key === `new:${newRequest.parent}`);
        if (virtualized && rowIndex >= 0) treeVirtualizer.scrollToIndex(rowIndex, { align: "auto" });
        requestAnimationFrame(() => newInputRef.current?.focus());
    }, [newRequest, treeVirtualizer, virtualized, visibleRows]);

    useEffect(() => {
        if (renaming) {
            const el = renameInputRef.current;
            if (el) {
                el.focus();
                const dot = el.value.lastIndexOf(".");
                if (dot > 0) el.setSelectionRange(0, dot);
                else el.select();
            }
        }
    }, [renaming]);

    const startRename = (entry: DirEntry) => {
        setRenaming(entry.path);
        setRenameName(entry.name);
    };
    const cancelRename = () => {
        setRenaming(null);
        setRenameName("");
    };
    const submitRename = async () => {
        if (!renaming) return;
        const trimmed = validEntryName(renameName);
        if (!trimmed) {
            notify("error", "name must be a single file or folder name");
            return;
        }
        if (trimmed === basename(renaming)) {
            cancelRename();
            return;
        }
        const parent = dirname(renaming);
        const dest = joinPath(parent, trimmed);
        try {
            await renameEditorPath(renaming, dest);
            await loadDir(parent);
            cancelRename();
        } catch (err) {
            reportError("rename")(err);
        }
    };

    const toggleDir = async (entry: DirEntry) => {
        setSelectedDir(entry.path);
        const open = expanded.has(entry.path);
        setExpanded((s) => {
            const n = new Set(s);
            open ? n.delete(entry.path) : n.add(entry.path);
            return n;
        });
        if (!open && !dirs[entry.path]) await loadDir(entry.path);
    };

    const startNew = (kind: "file" | "folder", parentOverride?: string) => {
        let parent = parentOverride ?? selectedDir;
        if (!parent && activePath && pathRelative(activePath, cwd) !== null) {
            const activeParent = dirname(activePath);
            if (activeParent !== cwd) parent = activeParent;
        }
        if (!parent) parent = cwd;
        setExpanded((s) => new Set(s).add(parent!));
        if (!dirs[parent]) void loadDir(parent);
        setNewRequest({ parent, kind });
        setNewName("");
    };

    const cancelNew = () => {
        setNewRequest(null);
        setNewName("");
    };

    const submitNew = async () => {
        if (!newRequest) return;
        const name = validEntryName(newName);
        if (!name) {
            notify("error", "name must be a single file or folder name");
            return;
        }
        const target = joinPath(newRequest.parent, name);
        try {
            if (newRequest.kind === "file") await fsapi.createFile(target);
            else await fsapi.createDir(target);
            await loadDir(newRequest.parent);
            cancelNew();
            if (newRequest.kind === "file") {
                onOpenFile({
                    name,
                    path: target,
                    is_dir: false,
                });
            }
        } catch (err) {
            reportError("create")(err);
        }
    };

    const folderDropRef = useRef<Map<string, { el: HTMLElement; unreg: () => void }>>(new Map());
    const folderDropVersionRef = useRef<Map<string, number>>(new Map());
    const attachFolderDrop = (el: HTMLButtonElement | null, dir: string) => {
        const version = (folderDropVersionRef.current.get(dir) ?? 0) + 1;
        folderDropVersionRef.current.set(dir, version);
        const existing = folderDropRef.current.get(dir);
        if (!el) {
            // Ref callbacks are inline, so React calls the old ref with null
            // before calling the new ref with the same element on re-render.
            // Defer cleanup; a same-tick reattach bumps the version and wins.
            queueMicrotask(() => {
                if (folderDropVersionRef.current.get(dir) !== version) return;
                folderDropRef.current.get(dir)?.unreg();
                folderDropRef.current.delete(dir);
                folderDropVersionRef.current.delete(dir);
            });
            return;
        }
        if (existing?.el === el) return;
        existing?.unreg();
        const unreg = registerFolderDrop(el, async (paths) => {
            try {
                for (const p of paths) await fsapi.copyIntoDir(p, dir);
                await loadDir(dir);
                setExpanded((s) => new Set(s).add(dir));
            } catch (err) {
                reportError("drop")(err);
            }
        });
        folderDropRef.current.set(dir, { el, unreg });
    };
    useEffect(() => {
        const map = folderDropRef.current;
        const versions = folderDropVersionRef.current;
        return () => {
            for (const entry of map.values()) entry.unreg();
            map.clear();
            versions.clear();
        };
    }, []);

    // Internal drag-and-drop: move a file/folder within the tree (VSCode-style).
    // External Finder drops are handled separately via registerFolderDrop above.
    const canDropInto = (src: string, destDir: string): boolean => {
        if (!src) return false;
        if (destDir === src) return false; // into itself
        if (isPathWithin(destDir, src)) return false; // into its own descendant
        if (dirname(src) === destDir) return false; // already lives there
        return true;
    };

    const moveEntry = async (src: string, destDir: string) => {
        if (!canDropInto(src, destDir)) return;
        const dest = joinPath(destDir, basename(src));
        try {
            await renameEditorPath(src, dest);
            await Promise.all([loadDir(dirname(src)), loadDir(destDir)]);
            setExpanded((s) => new Set(s).add(destDir));
        } catch (err) {
            reportError("move")(err);
        }
    };

    // Internal drag uses pointer events, NOT the HTML5 DnD API: Tauri's native OS
    // drag-drop handler (required for Finder→app drops) swallows dragover/drop on
    // macOS WKWebView, so in-app HTML5 dragging never fires its events. Pointer
    // events are fully under our control and immune to that.
    const dragSession = useRef<{ path: string; startX: number; startY: number; active: boolean } | null>(null);
    const moveHandlerRef = useRef<((e: PointerEvent) => void) | null>(null);
    const dragFrameRef = useRef<number | null>(null);
    const dragPointRef = useRef<{ x: number; y: number } | null>(null);
    const suppressClickRef = useRef(false);

    const updateDragOver = (next: string | null) => {
        setDragOver((prev) => (prev === next ? prev : next));
    };

    const updateRootDragOver = (next: boolean) => {
        setRootDragOver((prev) => (prev === next ? prev : next));
    };

    // What sits under (x, y): the destination dir + the folder/root to highlight.
    const resolveDrop = (x: number, y: number): { destDir: string | null; highlightPath: string | null } => {
        const at = document.elementFromPoint(x, y) as HTMLElement | null;
        if (!at?.closest(".ed-tree-scroll")) return { destDir: null, highlightPath: null };
        const rowEl = at.closest(".tree-row.is-folder, .tree-row.file") as HTMLElement | null;
        if (rowEl?.dataset.folderPath) {
            return { destDir: rowEl.dataset.folderPath, highlightPath: rowEl.dataset.folderPath };
        }
        if (rowEl?.dataset.filePath) {
            const destDir = dirname(rowEl.dataset.filePath);
            return { destDir, highlightPath: destDir === cwd ? null : destDir };
        }
        return { destDir: cwd, highlightPath: null };
    };

    const applyDragMove = () => {
        dragFrameRef.current = null;
        const point = dragPointRef.current;
        const s = dragSession.current;
        if (!s || !point) return;
        if (!s.active) {
            if (Math.hypot(point.x - s.startX, point.y - s.startY) < 5) return; // click vs drag threshold
            s.active = true;
            setDraggingPath(s.path);
        }
        const { destDir, highlightPath } = resolveDrop(point.x, point.y);
        const ok = destDir != null && canDropInto(s.path, destDir);
        updateDragOver(ok ? highlightPath : null);
        updateRootDragOver(ok && !highlightPath);
        const nextGhost = { name: basename(s.path), x: point.x, y: point.y };
        setDragGhost((prev) => (prev && prev.name === nextGhost.name && prev.x === nextGhost.x && prev.y === nextGhost.y ? prev : nextGhost));
    };

    const onDragMove = (e: PointerEvent) => {
        dragPointRef.current = { x: e.clientX, y: e.clientY };
        if (dragFrameRef.current == null) {
            dragFrameRef.current = window.requestAnimationFrame(applyDragMove);
        }
    };

    const endDrag = useCallback(() => {
        if (moveHandlerRef.current) window.removeEventListener("pointermove", moveHandlerRef.current);
        if (dragFrameRef.current != null) window.cancelAnimationFrame(dragFrameRef.current);
        moveHandlerRef.current = null;
        dragFrameRef.current = null;
        dragPointRef.current = null;
        dragSession.current = null;
        setDraggingPath(null);
        setDragOver(null);
        setRootDragOver(false);
        setDragGhost(null);
    }, []);

    const onDragUp = (e: PointerEvent) => {
        const s = dragSession.current;
        const active = !!s?.active;
        if (s && active) {
            const { destDir } = resolveDrop(e.clientX, e.clientY);
            if (destDir) void moveEntry(s.path, destDir);
        }
        endDrag();
        // Swallow the click that fires after a real drag. A click only fires when
        // pointer up/down share an element (drag back onto the source); dropping
        // elsewhere fires no click, so reset on the next tick to avoid eating a
        // later legit click.
        suppressClickRef.current = active;
        if (active) setTimeout(() => (suppressClickRef.current = false), 0);
    };

    const onRowPointerDown = (e: ReactPointerEvent, path: string) => {
        if (e.button !== 0) return; // left button only
        dragSession.current = { path, startX: e.clientX, startY: e.clientY, active: false };
        moveHandlerRef.current = onDragMove;
        window.addEventListener("pointermove", onDragMove);
        window.addEventListener("pointerup", onDragUp, { once: true });
    };

    // Defensive: drop the move listener if we unmount mid-drag.
    useEffect(() => () => endDrag(), [endDrag]);

    useEffect(() => {
        if (!active || !cwd) return;
        return subscribe("tree-native-drag-hover", (e) => {
            if (dragSession.current?.active) return;
            if (e.cwd !== cwd || !e.targetDir) {
                updateDragOver(null);
                updateRootDragOver(false);
                return;
            }
            updateDragOver(e.highlightPath);
            updateRootDragOver(!e.highlightPath);
        });
    }, [active, cwd]);

    // ---- right-click context menu -------------------------------------
    const relativePath = (p: string) => pathRelative(p, cwd) ?? basename(p);

    const copyText = async (text: string, label: string) => {
        try {
            await navigator.clipboard.writeText(text);
            notify("success", `copied ${label}`);
        } catch (err) {
            reportError("copy")(err);
        }
    };

    const revealInFinder = (p: string) => void fsapi.revealInFinder(p).catch(reportError("reveal"));

    const deleteEntry = async (entry: DirEntry) => {
        const ok = await confirmDialog({
            title: `Move "${entry.name}" to the Trash?`,
            body: entry.path,
            confirmLabel: "Move to Trash",
            destructive: true,
        });
        if (!ok) return;
        try {
            await fsapi.deletePath(entry.path);
            await loadDir(dirname(entry.path) || cwd);
            if (renaming === entry.path) cancelRename();
            if (selectedDir === entry.path) setSelectedDir(null);
            if (newRequest?.parent === entry.path) cancelNew();
        } catch (err) {
            reportError("delete")(err);
        }
    };

    const openMenu = (e: ReactMouseEvent, entry: DirEntry | null) => {
        e.preventDefault();
        e.stopPropagation();
        if (entry?.is_dir) setSelectedDir(entry.path);
        setMenu({ x: e.clientX, y: e.clientY, entry });
    };

    const buildMenuItems = (entry: DirEntry | null): CtxItem[] => {
        if (!entry) {
            return [
                { label: "New File", run: () => startNew("file", cwd) },
                { label: "New Folder", run: () => startNew("folder", cwd) },
                { sep: true },
                { label: `Reveal in ${FILE_MANAGER_NAME}`, run: () => revealInFinder(cwd) },
                { label: "Copy Path", run: () => void copyText(cwd, "path") },
            ];
        }
        const tail: CtxItem[] = [
            { label: `Reveal in ${FILE_MANAGER_NAME}`, run: () => revealInFinder(entry.path) },
            { label: "Copy Path", run: () => void copyText(entry.path, "path") },
            { label: "Copy Relative Path", run: () => void copyText(relativePath(entry.path), "relative path") },
        ];
        if (entry.is_dir) {
            return [
                { label: "New File", run: () => startNew("file", entry.path) },
                { label: "New Folder", run: () => startNew("folder", entry.path) },
                { sep: true },
                { label: "Rename…", run: () => startRename(entry) },
                { label: "Delete", danger: true, run: () => void deleteEntry(entry) },
                { sep: true },
                ...tail,
            ];
        }
        return [
            {
                label: "Open",
                run: () => {
                    setSelectedDir(null);
                    onOpenFile(entry);
                },
            },
            { sep: true },
            { label: "Rename…", run: () => startRename(entry) },
            { label: "Delete", danger: true, run: () => void deleteEntry(entry) },
            { sep: true },
            ...tail,
        ];
    };

    const onEntryKey = (event: React.KeyboardEvent<HTMLButtonElement>, entry: DirEntry) => {
        if (event.key === "F2") {
            event.preventDefault();
            startRename(entry);
        }
        if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
            event.preventDefault();
            const rect = event.currentTarget.getBoundingClientRect();
            setMenu({ x: rect.left, y: rect.bottom, entry });
        }
    };

    const attachRowButton = (path: string, el: HTMLButtonElement | null) => {
        if (el) rowButtonRefs.current.set(path, el);
        else rowButtonRefs.current.delete(path);
    };

    const renderRow = (row: VisibleTreeRow): ReactNode => {
        if (row.kind === "new") {
            return (
                <NewEntryRow
                    depth={row.depth}
                    kind={row.request.kind}
                    value={newName}
                    inputRef={newInputRef}
                    onChange={setNewName}
                    onSubmit={submitNew}
                    onCancel={cancelNew}
                />
            );
        }
        const e = row.entry;
        const pad = 10 + row.depth * 13;
        if (renaming === e.path) {
            return (
                <RenameRow
                    depth={row.depth + (e.is_dir ? 0 : 1)}
                    kind={e.is_dir ? "folder" : "file"}
                    value={renameName}
                    inputRef={renameInputRef}
                    onChange={setRenameName}
                    onSubmit={submitRename}
                    onCancel={cancelRename}
                />
            );
        }
        if (e.is_dir) {
            const open = expanded.has(e.path);
            return (
                <button
                    ref={(el) => {
                        attachFolderDrop(el, e.path);
                        attachRowButton(e.path, el);
                    }}
                    className={`tree-row is-folder${selectedDir === e.path ? " selected" : ""}${dragOver === e.path ? " drag-over" : ""}${draggingPath === e.path ? " dragging" : ""}`}
                    style={{ paddingLeft: pad }}
                    onPointerDown={(ev) => onRowPointerDown(ev, e.path)}
                    onDragStart={(ev) => ev.preventDefault()}
                    onClick={() => {
                        if (suppressClickRef.current) {
                            suppressClickRef.current = false;
                            return;
                        }
                        void toggleDir(e);
                    }}
                    onContextMenu={(ev) => openMenu(ev, e)}
                    role="treeitem"
                    aria-expanded={open}
                    aria-level={row.depth + 1}
                    aria-selected={selectedDir === e.path}
                    tabIndex={focusPath === e.path ? 0 : -1}
                    onFocus={() => setFocusedPath(e.path)}
                    onKeyDown={(event) => onEntryKey(event, e)}
                    data-folder-path={e.path}>
                    <span className={`tree-chev${open ? " open" : ""}`}>
                        <IconChevron size={11} />
                    </span>
                    <span className="tree-folder">
                        <IconFolder size={17} />
                    </span>
                    <span className="tree-name">{e.name}</span>
                </button>
            );
        }
        const gf = gitMap.get(normalizePath(e.path));
        const gd = gf ? gitFileDecoration(gf) : null;
        return (
            <button
                ref={(el) => attachRowButton(e.path, el)}
                className={`tree-row file${activePath === e.path ? " active" : ""}${gd ? ` git-${gd.cls}` : ""}${dragOver === e.path ? " drag-over" : ""}${draggingPath === e.path ? " dragging" : ""}`}
                style={{ paddingLeft: pad + 13 }}
                onPointerDown={(ev) => onRowPointerDown(ev, e.path)}
                onDragStart={(ev) => ev.preventDefault()}
                onClick={() => {
                    if (suppressClickRef.current) {
                        suppressClickRef.current = false;
                        return;
                    }
                    setSelectedDir(null);
                    onOpenFile(e);
                }}
                onContextMenu={(ev) => openMenu(ev, e)}
                role="treeitem"
                aria-level={row.depth + 1}
                aria-selected={activePath === e.path}
                tabIndex={focusPath === e.path ? 0 : -1}
                onFocus={() => setFocusedPath(e.path)}
                onKeyDown={(event) => onEntryKey(event, e)}
                data-file-path={e.path}
                data-drop-dir={dirname(e.path)}>
                <span className="tree-file">
                    <FileIcon name={e.name} size={20} />
                </span>
                <span className="tree-name">{e.name}</span>
                {gd && <span className="tree-git">{gd.letter}</span>}
            </button>
        );
    };

    const onResizeDrag = (e: ReactPointerEvent<HTMLDivElement>) => {
        e.preventDefault();
        const handle = e.currentTarget;
        handle.setPointerCapture(e.pointerId);
        const startX = e.clientX;
        const startW = width ?? 0;
        let latest = startW;
        let frame: number | null = null;
        const move = (ev: PointerEvent) => {
            latest = Math.min(600, Math.max(160, startW + ev.clientX - startX));
            if (frame == null) {
                frame = window.requestAnimationFrame(() => {
                    frame = null;
                    onResize?.(latest);
                });
            }
        };
        const up = () => {
            if (frame != null) {
                window.cancelAnimationFrame(frame);
                frame = null;
            }
            onResize?.(latest);
            handle.removeEventListener("pointermove", move);
            handle.removeEventListener("pointerup", up);
        };
        handle.addEventListener("pointermove", move);
        handle.addEventListener("pointerup", up);
    };

    const onResizeKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
        if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
        e.preventDefault();
        const step = e.shiftKey ? 40 : 16;
        onResize?.(Math.min(600, Math.max(160, (width ?? 0) + (e.key === "ArrowRight" ? step : -step))));
    };

    useEffect(() => {
        const el = rootScrollRef.current;
        if (!el || !cwd || !active) return;
        return registerFolderDrop(el, async (paths) => {
            try {
                for (const p of paths) await fsapi.copyIntoDir(p, cwd);
                await loadDir(cwd);
            } catch (err) {
                reportError("drop")(err);
            }
        });
    }, [cwd, active, loadDir]);

    const focusEntryAt = (entryIndex: number) => {
        const row = entryRows[entryIndex];
        if (!row) return;
        setFocusedPath(row.entry.path);
        const visibleIndex = visibleRows.indexOf(row);
        if (virtualized) treeVirtualizer.scrollToIndex(visibleIndex, { align: "auto" });
        requestAnimationFrame(() => rowButtonRefs.current.get(row.entry.path)?.focus());
    };

    const renderedRows = virtualized
        ? treeVirtualizer.getVirtualItems().map((item) => {
              const row = visibleRows[item.index];
              return (
                  <div
                      key={row.key}
                      style={{ position: "absolute", top: 0, left: 0, width: "100%", height: item.size, transform: `translateY(${item.start}px)` }}>
                      {renderRow(row)}
                  </div>
              );
          })
        : visibleRows.map((row) => <div key={row.key}>{renderRow(row)}</div>);

    return (
        <>
            <div className={`ed-tree${resizable ? "" : " fill"}`} style={resizable ? { width } : undefined}>
                <div className="ed-tree-head">
                    <span className="ed-tree-name">{basename(cwd) || "files"}</span>
                    <span className="ed-tree-actions">
                        <Tooltip label="New file">
                            <button type="button" className="ed-tree-act" aria-label="New file" onClick={() => startNew("file")}>
                                <FileIcon name="" size={13} />
                                <IconPlus size={9} />
                            </button>
                        </Tooltip>
                        <Tooltip label="New folder">
                            <button type="button" className="ed-tree-act" aria-label="New folder" onClick={() => startNew("folder")}>
                                <IconFolder size={13} />
                                <IconPlus size={9} />
                            </button>
                        </Tooltip>
                    </span>
                </div>
                <div
                    ref={rootScrollRef}
                    className={`ed-tree-scroll${rootDragOver ? " drag-over-root" : ""}`}
                    role="tree"
                    aria-label="Project files"
                    onKeyDown={(event) => {
                        const target = event.target as HTMLElement;
                        if (target.getAttribute("role") !== "treeitem") return;
                        const path = target.dataset.folderPath ?? target.dataset.filePath;
                        const index = entryRows.findIndex((row) => row.entry.path === path);
                        if (index < 0) return;
                        const row = entryRows[index];
                        const isExpanded = row.entry.is_dir && expanded.has(row.entry.path);
                        let next = index;
                        if (event.key === "ArrowDown") next = Math.min(entryRows.length - 1, index + 1);
                        else if (event.key === "ArrowUp") next = Math.max(0, index - 1);
                        else if (event.key === "Home") next = 0;
                        else if (event.key === "End") next = entryRows.length - 1;
                        else if (event.key === "ArrowRight") {
                            if (row.entry.is_dir && !isExpanded) target.click();
                            else if (isExpanded) next = Math.min(entryRows.length - 1, index + 1);
                        } else if (event.key === "ArrowLeft") {
                            if (isExpanded) target.click();
                            else {
                                const level = row.depth;
                                for (let i = index - 1; i >= 0; i--)
                                    if (entryRows[i].depth < level) {
                                        next = i;
                                        break;
                                    }
                            }
                        } else return;
                        event.preventDefault();
                        event.stopPropagation();
                        focusEntryAt(next);
                    }}
                    data-root-path={cwd}
                    onContextMenu={(ev) => openMenu(ev, null)}>
                    <div style={virtualized ? { position: "relative", width: "100%", height: treeVirtualizer.getTotalSize() } : undefined}>
                        {renderedRows}
                    </div>
                </div>
            </div>
            {resizable && (
                <div
                    className="ed-tree-resizer"
                    role="separator"
                    tabIndex={0}
                    aria-orientation="vertical"
                    aria-valuemin={160}
                    aria-valuemax={600}
                    aria-valuenow={Math.round(width)}
                    onPointerDown={onResizeDrag}
                    onKeyDown={onResizeKeyDown}
                    title="Drag or use arrow keys to resize"
                />
            )}
            {menu && <TreeContextMenu x={menu.x} y={menu.y} items={buildMenuItems(menu.entry)} onClose={() => setMenu(null)} />}
            {dragGhost &&
                createPortal(
                    <div className="tree-drag-ghost" style={{ left: dragGhost.x + 12, top: dragGhost.y + 10 }}>
                        {dragGhost.name}
                    </div>,
                    document.body,
                )}
        </>
    );
}

export function TreeContextMenu({ x, y, items, onClose }: { x: number; y: number; items: CtxItem[]; onClose: () => void }) {
    const ref = useRef<HTMLDivElement>(null);
    const [pos, setPos] = useState({ left: x, top: y });

    useLayoutEffect(() => {
        const el = ref.current;
        if (!el) return;
        const r = el.getBoundingClientRect();
        const pad = 6;
        let left = x;
        let top = y;
        if (left + r.width > window.innerWidth - pad) left = Math.max(pad, window.innerWidth - r.width - pad);
        if (top + r.height > window.innerHeight - pad) top = Math.max(pad, window.innerHeight - r.height - pad);
        setPos({ left, top });
    }, [x, y]);

    useLayoutEffect(() => {
        const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        ref.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
        return () => {
            if (previous?.isConnected) previous.focus();
        };
    }, []);

    return createPortal(
        <div
            className="tree-ctx-scrim"
            onClick={onClose}
            onContextMenu={(e) => {
                e.preventDefault();
                onClose();
            }}>
            <div
                ref={ref}
                role="menu"
                aria-label="Actions"
                onKeyDown={(event) => {
                    const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
                    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
                    let next = index;
                    if (event.key === "ArrowDown") next = (index + 1) % buttons.length;
                    else if (event.key === "ArrowUp") next = (index - 1 + buttons.length) % buttons.length;
                    else if (event.key === "Home") next = 0;
                    else if (event.key === "End") next = buttons.length - 1;
                    else if (event.key === "Escape" || event.key === "Tab") {
                        event.preventDefault();
                        event.stopPropagation();
                        onClose();
                        return;
                    } else return;
                    event.preventDefault();
                    event.stopPropagation();
                    buttons[next]?.focus();
                }}
                className="tree-ctx-menu"
                style={{ left: pos.left, top: pos.top }}
                onClick={(e) => e.stopPropagation()}>
                {items.map((it, i) =>
                    it.sep ? (
                        <div key={i} className="tree-ctx-sep" />
                    ) : (
                        <button
                            key={i}
                            type="button"
                            disabled={it.disabled}
                            role="menuitem"
                            tabIndex={-1}
                            className={`tree-ctx-item${it.danger ? " danger" : ""}${it.disabled ? " disabled" : ""}`}
                            onClick={() => {
                                if (it.disabled) return;
                                onClose();
                                it.run?.();
                            }}>
                            <span className="tree-ctx-label">{it.label}</span>
                            {it.hint && <span className="tree-ctx-hint">{it.hint}</span>}
                        </button>
                    ),
                )}
            </div>
        </div>,
        document.body,
    );
}

function NewEntryRow({
    depth,
    kind,
    value,
    onChange,
    onSubmit,
    onCancel,
    inputRef,
}: {
    depth: number;
    kind: "file" | "folder";
    value: string;
    onChange: (v: string) => void;
    onSubmit: () => void;
    onCancel: () => void;
    inputRef: React.RefObject<HTMLInputElement | null>;
}) {
    const pad = 10 + depth * 13;
    return (
        <div className="tree-row tree-new" style={{ paddingLeft: pad + 13 }}>
            <span className="tree-file">{kind === "folder" ? <IconFolder size={17} /> : <FileIcon name="" size={20} />}</span>
            <input
                ref={inputRef}
                className="tree-new-input"
                placeholder={kind === "folder" ? "folder name…" : "filename…"}
                value={value}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                onChange={(e) => onChange(e.target.value)}
                onBlur={onCancel}
                onKeyDown={(e) => {
                    if (e.key === "Enter") onSubmit();
                    else if (e.key === "Escape") onCancel();
                    e.stopPropagation();
                }}
            />
        </div>
    );
}

function RenameRow({
    depth,
    kind,
    value,
    onChange,
    onSubmit,
    onCancel,
    inputRef,
}: {
    depth: number;
    kind: "file" | "folder";
    value: string;
    onChange: (v: string) => void;
    onSubmit: () => void;
    onCancel: () => void;
    inputRef: React.RefObject<HTMLInputElement | null>;
}) {
    const pad = 10 + depth * 13;
    return (
        <div className="tree-row tree-new" style={{ paddingLeft: pad }}>
            {kind === "folder" ? (
                <>
                    <span className="tree-chev" style={{ visibility: "hidden" }}>
                        <IconChevron size={11} />
                    </span>
                    <span className="tree-folder">
                        <IconFolder size={17} />
                    </span>
                </>
            ) : (
                <span className="tree-file">
                    <FileIcon name={value} size={20} />
                </span>
            )}
            <input
                ref={inputRef}
                className="tree-new-input"
                value={value}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                onChange={(e) => onChange(e.target.value)}
                onBlur={onCancel}
                onKeyDown={(e) => {
                    if (e.key === "Enter") onSubmit();
                    else if (e.key === "Escape") onCancel();
                    e.stopPropagation();
                }}
            />
        </div>
    );
}
