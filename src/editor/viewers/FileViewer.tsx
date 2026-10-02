import type { FilePreview } from "../../api/fs";
import { DocumentView } from "./DocumentView";
import { FontView } from "./FontView";
import { HexView } from "./HexView";
import { ImageView } from "./ImageView";
import { MediaView } from "./MediaView";
import { PdfView } from "./PdfView";
import { previewUrl, viewerKind } from "./fileKinds";
import { ViewerBar, ViewerMessage } from "./ViewerBar";

export interface ViewerState {
    path: string;
    /** Bumped when the file changes on disk, so the window loads it again. */
    revision: number;
    preview?: FilePreview;
    error?: string;
}

export default function FileViewer({ viewer, visible, onReload }: { viewer: ViewerState; visible: boolean; onReload: (path: string) => void }) {
    const { path, preview, error } = viewer;
    if (!preview) {
        return (
            <div className="ed-viewer">
                <ViewerBar path={path} onReload={onReload} />
                <div className="ed-viewer-stage">
                    {error ? <ViewerMessage title="couldn't open file" detail={error} error /> : <ViewerMessage title="loading…" />}
                </div>
            </div>
        );
    }
    const url = previewUrl(path, viewer.revision);
    const props = { path, preview, url, onReload };
    const kind = viewerKind(preview.mime);
    switch (kind) {
        case "image":
            return <ImageView key={url} {...props} />;
        case "video":
        case "audio":
            return <MediaView key={url} kind={kind} visible={visible} {...props} />;
        case "pdf":
            return <PdfView key={url} {...props} />;
        case "font":
            return <FontView key={url} {...props} />;
        case "document":
            return <DocumentView key={url} visible={visible} {...props} />;
        case "binary":
            return <HexView key={url} {...props} />;
    }
}
