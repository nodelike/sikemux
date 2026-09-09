import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { DiffEditor } from "./DiffEditor";
import { FileIcon } from "./FileIcon";
import { IconChevron } from "./Icons";
import { Tooltip } from "./Tooltip";
import { hasUnstaged, isStaged, type GitFile } from "../api/git";
import { basename, joinPath } from "../lib/paths";
import { gitStatusDecoration, type GitStatusDecoration } from "./git/gitFileStatus";

const VIRTUAL_REVIEW_THRESHOLD = 8;
const REVIEW_ROW_ESTIMATE = 250;
const REVIEW_DOUBLE_ROW_ESTIMATE = 470;
const REVIEW_HEADER_HEIGHT = 31;

export function MergeReview({
    repo,
    files,
    focusPath,
    onOpenFile,
    onSaved,
}: {
    repo: string;
    files: GitFile[];
    focusPath?: string;
    onOpenFile: (abs: string) => void;
    onSaved: () => void;
}) {
    const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
    const itemRefs = useRef(new Map<string, HTMLDivElement>());
    const listRef = useRef<HTMLDivElement>(null);
    const paths = useMemo(() => files.map((file) => file.path), [files]);
    const pathSet = useMemo(() => new Set(paths), [paths]);
    const pathIndex = useMemo(() => new Map(paths.map((path, index) => [path, index])), [paths]);
    const virtual = files.length > VIRTUAL_REVIEW_THRESHOLD;
    const virtualizer = useVirtualizer({
        count: virtual ? files.length : 0,
        getScrollElement: () => listRef.current,
        estimateSize: (index) => {
            const file = files[index];
            if (!file || collapsed.has(file.path)) return REVIEW_HEADER_HEIGHT;
            return isStaged(file) && hasUnstaged(file) ? REVIEW_DOUBLE_ROW_ESTIMATE : REVIEW_ROW_ESTIMATE;
        },
        getItemKey: (index) => files[index]?.path ?? index,
        overscan: 2,
        initialRect: { width: 1000, height: 800 },
    });

    useEffect(() => {
        setCollapsed((current) => {
            const next = new Set([...current].filter((path) => pathSet.has(path)));
            return next.size === current.size ? current : next;
        });
    }, [pathSet]);

    useEffect(() => {
        if (virtual) virtualizer.measure();
    }, [collapsed, files, virtual, virtualizer]);

    useEffect(() => {
        if (!focusPath || !pathSet.has(focusPath)) return;
        setCollapsed((current) => {
            if (!current.has(focusPath)) return current;
            const next = new Set(current);
            next.delete(focusPath);
            return next;
        });
        const index = pathIndex.get(focusPath) ?? -1;
        window.requestAnimationFrame(() => {
            if (virtual && index >= 0) virtualizer.scrollToIndex(index, { align: "start" });
            else itemRefs.current.get(focusPath)?.scrollIntoView?.({ block: "start" });
        });
    }, [focusPath, pathIndex, pathSet, virtual, virtualizer]);

    const toggle = (path: string) => {
        setCollapsed((current) => {
            const next = new Set(current);
            next.has(path) ? next.delete(path) : next.add(path);
            return next;
        });
    };

    const expandedCount = paths.filter((path) => !collapsed.has(path)).length;

    return (
        <div className="merge-review">
            <div className="merge-review-toolbar">
                <span className="merge-review-count">
                    {files.length} {files.length === 1 ? "file" : "files"} · {expandedCount} expanded
                </span>
                <button
                    type="button"
                    className="merge-review-action"
                    onClick={() => setCollapsed(new Set())}
                    disabled={expandedCount === files.length}>
                    expand all
                </button>
                <button type="button" className="merge-review-action" onClick={() => setCollapsed(new Set(paths))} disabled={expandedCount === 0}>
                    collapse all
                </button>
            </div>
            <div className="merge-review-list" ref={listRef}>
                {virtual ? (
                    <div className="merge-review-virtual" style={{ height: virtualizer.getTotalSize() }}>
                        {virtualizer.getVirtualItems().map((row) => {
                            const file = files[row.index];
                            if (!file) return null;
                            const style: CSSProperties = { transform: `translateY(${row.start}px)` };
                            return (
                                <div
                                    key={row.key}
                                    ref={virtualizer.measureElement}
                                    data-index={row.index}
                                    className="merge-review-virtual-item"
                                    style={style}>
                                    {renderFile(file)}
                                </div>
                            );
                        })}
                    </div>
                ) : (
                    files.map((file) => renderFile(file))
                )}
            </div>
        </div>
    );

    function renderFile(file: GitFile) {
        const path = file.path;
        const open = !collapsed.has(path);
        const focused = path === focusPath;
        const unstaged = hasUnstaged(file);
        const indexStatus = gitStatusDecoration(file.index);
        const worktreeStatus = gitStatusDecoration(file.worktree);
        return (
            <div
                className={`acc-item merge-review-item${focused ? " focused" : ""}`}
                key={path}
                ref={(node) => {
                    if (node) itemRefs.current.set(path, node);
                    else itemRefs.current.delete(path);
                }}>
                <div className="acc-header merge-file-header">
                    <Tooltip label={open ? "Collapse" : "Expand"}>
                        <button
                            type="button"
                            className="acc-toggle"
                            onClick={() => toggle(path)}
                            aria-label={`${open ? "Collapse" : "Expand"} ${path}`}>
                            <span className={`acc-chev${open ? " open" : ""}`}>
                                <IconChevron size={11} />
                            </span>
                        </button>
                    </Tooltip>
                    <Tooltip label="Open in editor">
                        <button type="button" className="acc-name" onClick={() => onOpenFile(joinPath(repo, path))}>
                            <FileIcon name={basename(path)} size={15} />
                            <span>{path}</span>
                        </button>
                    </Tooltip>
                    <span className="merge-file-status">
                        <GitStatusSymbol status={indexStatus} source="Index" />
                        <GitStatusSymbol status={worktreeStatus} source="Working tree" />
                    </span>
                </div>
                {open && <MergeFileDiff repo={repo} file={file} editable={focused && unstaged} onSaved={onSaved} />}
            </div>
        );
    }
}

function GitStatusSymbol({ status, source }: { status: GitStatusDecoration | null; source: string }) {
    if (!status) return null;
    return (
        <span
            className={`git-status-symbol git-${status.cls}`}
            title={`${source}: ${status.label}`}
            aria-label={`${source} status: ${status.letter}`}>
            {status.letter}
        </span>
    );
}

function MergeFileDiff({ repo, file, editable, onSaved }: { repo: string; file: GitFile; editable: boolean; onSaved: () => void }) {
    const path = file.path;
    const staged = isStaged(file);
    const unstaged = hasUnstaged(file);
    const indexStatus = gitStatusDecoration(file.index);
    const worktreeStatus = gitStatusDecoration(file.worktree);

    return (
        <div className="merge-review-content">
            {staged && unstaged ? (
                <div className="merge-sections">
                    <div className="merge-section">
                        <div className="merge-section-title">
                            <GitStatusSymbol status={indexStatus} source="Index" />
                        </div>
                        <DiffEditor repo={repo} path={path} baseRev="HEAD" headRev=":index" editable={false} autoHeight />
                    </div>
                    <div className="merge-section">
                        <div className="merge-section-title">
                            <GitStatusSymbol status={worktreeStatus} source="Working tree" />
                        </div>
                        <DiffEditor repo={repo} path={path} baseRev=":index" editable={editable} onSaved={onSaved} autoHeight />
                    </div>
                </div>
            ) : staged ? (
                <DiffEditor repo={repo} path={path} baseRev="HEAD" headRev=":index" editable={false} autoHeight />
            ) : (
                <DiffEditor repo={repo} path={path} baseRev="HEAD" editable={editable} onSaved={onSaved} autoHeight />
            )}
        </div>
    );
}
