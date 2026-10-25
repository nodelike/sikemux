import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { CardStream, type CardStreamHandle } from "../ui/CardStream";
import { FileIcon } from "../ui/FileIcon";
import { IconCollapseAll, IconExpandAll } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";
import { basename, dirname } from "@shared/lib/paths";

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
    /** Folds every file whenever a new set of files arrives, as a commit's list does. */
    startCollapsed?: boolean;
    /** Folding every file from outside, by a control that lives elsewhere; the list then shows no toolbar of its own. */
    foldAll?: { folded: boolean; seq: number };
}

/**
 * Changed files in one collapsible, virtualized stream: a header per file with its icon, path and status, and its diff
 * under it. Local changes, commits and pull requests are all reviewed through it.
 */
export function FileReviewList({
    paths,
    focusPath,
    estimate,
    onOpenFile,
    status,
    actions,
    body,
    startCollapsed = false,
    foldAll,
}: FileReviewListProps) {
    const [collapsed, setCollapsed] = useState<Set<string>>(() => (startCollapsed ? new Set(paths) : new Set()));
    const stream = useRef<CardStreamHandle>(null);
    const pathSet = useMemo(() => new Set(paths), [paths]);
    const pathIndex = useMemo(() => new Map(paths.map((path, index) => [path, index])), [paths]);

    useEffect(() => {
        setCollapsed((current) => {
            if (startCollapsed) return new Set(pathSet);
            const next = new Set([...current].filter((path) => pathSet.has(path)));
            return next.size === current.size ? current : next;
        });
    }, [pathSet, startCollapsed]);

    // Only a new request folds; a changed file list keeps what the reader opened.
    const listed = useRef(pathSet);
    listed.current = pathSet;
    const foldSeq = foldAll?.seq ?? 0;
    const foldedAll = foldAll?.folded ?? false;
    useEffect(() => {
        if (foldSeq === 0) return;
        setCollapsed(foldedAll ? new Set(listed.current) : new Set());
    }, [foldSeq, foldedAll]);

    const scrollTargets = useRef(pathIndex);
    scrollTargets.current = pathIndex;
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
        const row = scrollTargets.current.get(focusPath) ?? -1;
        window.requestAnimationFrame(() => {
            if (row >= 0) stream.current?.scrollToCard(row);
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
            {!foldAll && (
                <div className="merge-review-toolbar">
                    <span className="merge-review-count">
                        {paths.length} {paths.length === 1 ? "file" : "files"}
                    </span>
                    <button
                        type="button"
                        className="merge-review-action"
                        onClick={() => setCollapsed(new Set())}
                        disabled={expandedCount === paths.length}>
                        <IconExpandAll size={12} />
                        Expand all
                    </button>
                    <button type="button" className="merge-review-action" onClick={() => setCollapsed(new Set(paths))} disabled={expandedCount === 0}>
                        <IconCollapseAll size={12} />
                        Collapse all
                    </button>
                </div>
            )}
            <CardStream
                handle={stream}
                keys={paths}
                collapsed={collapsed}
                onToggle={toggle}
                estimate={estimate}
                toggleLabel={(path, open) => `${open ? "Collapse" : "Expand"} ${path}`}
                itemClassName={(path) => (path === focusPath ? "focused" : "")}
                header={renderHeader}
                body={(path) => body(path, path === focusPath)}
            />
        </div>
    );

    function renderHeader(path: string) {
        const name = (
            <>
                {dirname(path) && <span className="merge-file-dir">{dirname(path)}/</span>}
                <span className="merge-file-base">{basename(path)}</span>
            </>
        );
        return (
            <>
                <FileIcon name={basename(path)} size={14} />
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
            </>
        );
    }
}
