import { useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { FileIcon } from "../ui/FileIcon";
import { IconChevron } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";
import { basename, dirname } from "../lib/paths";

const REVIEW_HEADER_HEIGHT = 29;

export interface FileReviewListProps {
    /** Repository-relative paths, in the order they are listed. */
    paths: readonly string[];
    focusPath?: string;
    /** How tall a file's open diff is likely to be, before it is measured. */
    estimate: (path: string) => number;
    /** Opens the file in the editor; without it the name is plain text. */
    onOpenFile?: (path: string) => void;
    /** Status letters or counts at the end of a file's header. */
    status: (path: string) => ReactNode;
    /** Buttons shown in a file's header while the pointer is on it. */
    actions?: (path: string) => ReactNode;
    /** The diff under a file's header, drawn only while that file is open and near the view. */
    body: (path: string, focused: boolean) => ReactNode;
}

/**
 * Changed files in one collapsible, virtualized stream: a header per file with its icon, path and status, and its diff
 * under it. Local changes, commits and pull requests are all reviewed through it.
 */
export function FileReviewList({ paths, focusPath, estimate, onOpenFile, status, actions, body }: FileReviewListProps) {
    const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
    const itemRefs = useRef(new Map<string, HTMLDivElement>());
    const listRef = useRef<HTMLDivElement>(null);
    const pathSet = useMemo(() => new Set(paths), [paths]);
    const pathIndex = useMemo(() => new Map(paths.map((path, index) => [path, index])), [paths]);
    const virtualizer = useVirtualizer({
        count: paths.length,
        getScrollElement: () => listRef.current,
        estimateSize: (index) => {
            const path = paths[index];
            if (path === undefined || collapsed.has(path)) return REVIEW_HEADER_HEIGHT;
            return estimate(path);
        },
        getItemKey: (index) => paths[index] ?? index,
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
        virtualizer.measure();
    }, [collapsed, paths, virtualizer]);

    const scrollTargets = useRef({ pathIndex, virtualizer });
    scrollTargets.current = { pathIndex, virtualizer };
    const focusListed = !!focusPath && pathSet.has(focusPath);

    // Only a new focus (or one that has just appeared in the list) scrolls. A
    // status refresh must leave the reader where they were.
    useEffect(() => {
        if (!focusPath || !focusListed) return;
        setCollapsed((current) => {
            if (!current.has(focusPath)) return current;
            const next = new Set(current);
            next.delete(focusPath);
            return next;
        });
        const { pathIndex: index, virtualizer: list } = scrollTargets.current;
        const row = index.get(focusPath) ?? -1;
        window.requestAnimationFrame(() => {
            if (row >= 0) list.scrollToIndex(row, { align: "start" });
            else itemRefs.current.get(focusPath)?.scrollIntoView?.({ block: "start" });
        });
    }, [focusPath, focusListed]);

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
                    {paths.length} {paths.length === 1 ? "file" : "files"}
                </span>
                <button
                    type="button"
                    className="merge-review-action"
                    onClick={() => setCollapsed(new Set())}
                    disabled={expandedCount === paths.length}>
                    Expand all
                </button>
                <button type="button" className="merge-review-action" onClick={() => setCollapsed(new Set(paths))} disabled={expandedCount === 0}>
                    Collapse all
                </button>
            </div>
            <div className="merge-review-list" ref={listRef}>
                <div className="merge-review-virtual" style={{ height: virtualizer.getTotalSize() }}>
                    {virtualizer.getVirtualItems().map((row) => {
                        const path = paths[row.index];
                        if (path === undefined) return null;
                        // Placed by `top`, not a transform: WebKit draws a sticky file header inside a transformed box at the box's bottom and leaves it there as the list scrolls.
                        const style: CSSProperties = {
                            top: row.start,
                            height: row.size,
                            overflow: "clip",
                        };
                        return (
                            <div key={row.key} className="merge-review-virtual-item" style={style}>
                                <div ref={virtualizer.measureElement} data-index={row.index}>
                                    {renderFile(path)}
                                </div>
                            </div>
                        );
                    })}
                </div>
            </div>
        </div>
    );

    function renderFile(path: string) {
        const open = !collapsed.has(path);
        const focused = path === focusPath;
        const name = (
            <>
                {dirname(path) && <span className="merge-file-dir">{dirname(path)}/</span>}
                <span className="merge-file-base">{basename(path)}</span>
            </>
        );
        return (
            <div
                className={`acc-item merge-review-item${open ? " open" : ""}${focused ? " focused" : ""}`}
                key={path}
                ref={(node) => {
                    if (node) itemRefs.current.set(path, node);
                    else itemRefs.current.delete(path);
                }}>
                <div className="acc-header merge-file-header" onClick={() => toggle(path)}>
                    <Tooltip label={open ? "Collapse" : "Expand"}>
                        <button type="button" className="acc-toggle" aria-label={`${open ? "Collapse" : "Expand"} ${path}`}>
                            <span className={`acc-chev${open ? " open" : ""}`}>
                                <IconChevron size={11} />
                            </span>
                        </button>
                    </Tooltip>
                    <FileIcon name={basename(path)} size={15} />
                    {onOpenFile ? (
                        <Tooltip label="Open in editor">
                            <button
                                type="button"
                                className="acc-name"
                                onClick={(event) => {
                                    event.stopPropagation();
                                    onOpenFile(path);
                                }}>
                                {name}
                            </button>
                        </Tooltip>
                    ) : (
                        <span className="acc-name">{name}</span>
                    )}
                    <span className="acc-grow" />
                    {actions && <span className="merge-file-actions">{actions(path)}</span>}
                    <span className="merge-file-status">{status(path)}</span>
                </div>
                {open && body(path, focused)}
            </div>
        );
    }
}
