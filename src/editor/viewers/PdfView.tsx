import { basename } from "../../lib/paths";
import { ViewerBar, type ViewProps } from "./ViewerBar";

export function PdfView({ path, preview, url, onReload }: ViewProps) {
    return (
        <div className="ed-viewer">
            <ViewerBar path={path} preview={preview} onReload={onReload} />
            <iframe className="ed-viewer-frame" src={url} title={basename(path)} />
        </div>
    );
}
