import type { ReactNode } from "react";
import { fsapi, type FilePreview } from "../../api/fs";
import { reportError } from "../../state/toast";
import { FileIcon } from "../../ui/FileIcon";
import { basename } from "@shared/lib/paths";
import { formatBytes } from "@shared/lib/format";

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
                <FileIcon name={basename(path)} size={14} />
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
