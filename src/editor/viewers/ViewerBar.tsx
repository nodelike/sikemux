import type { ReactNode } from "react";
import { fsapi, type FilePreview } from "../../api/fs";
import { reportError } from "../../state/toast";
import { FileIcon } from "../../ui/FileIcon";
import { basename } from "../../lib/paths";

export function formatBytes(n: number): string {
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

export function ViewerBar({
    path,
    preview,
    meta,
    actions,
    onReload,
}: {
    path: string;
    preview?: FilePreview;
    meta?: ReactNode;
    actions?: ReactNode;
    onReload: (path: string) => void;
}) {
    return (
        <div className="ed-viewer-bar">
            <div className="ed-viewer-title" title={path}>
                <FileIcon name={basename(path)} size={16} />
                <span>{basename(path)}</span>
            </div>
            <div className="ed-viewer-meta">
                {preview && <span>{formatBytes(preview.size)}</span>}
                {meta}
                {preview && <span>{preview.mime}</span>}
            </div>
            <div className="ed-viewer-actions">
                {actions}
                <button type="button" onClick={() => onReload(path)} title="Reload from disk">
                    reload
                </button>
                <button
                    type="button"
                    onClick={() => void fsapi.openInDefaultApp(path).catch(reportError("open file"))}
                    title="Open in the default app">
                    open
                </button>
                <button type="button" onClick={() => void fsapi.revealInFinder(path).catch(reportError("reveal file"))} title="Reveal in Finder">
                    reveal
                </button>
            </div>
        </div>
    );
}

export function ViewerMessage({ title, detail, error = false }: { title: string; detail?: string; error?: boolean }) {
    return (
        <div className={`ed-viewer-message${error ? " error" : ""}`}>
            <strong>{title}</strong>
            {detail && <span>{detail}</span>}
        </div>
    );
}

export interface ViewProps {
    path: string;
    preview: FilePreview;
    url: string;
    onReload: (path: string) => void;
}
