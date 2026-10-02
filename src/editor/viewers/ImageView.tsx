import { useState } from "react";
import { basename } from "../../lib/paths";
import { ViewerBar, ViewerMessage, type ViewProps } from "./ViewerBar";

export function ImageView({ path, preview, url, onReload }: ViewProps) {
    const [dims, setDims] = useState<{ w: number; h: number } | null>(null);
    const [zoom, setZoom] = useState<"fit" | number>("fit");
    const [failed, setFailed] = useState(false);
    const zoomLabel = zoom === "fit" ? "fit" : `${Math.round(zoom * 100)}%`;

    return (
        <div className="ed-viewer">
            <ViewerBar
                path={path}
                preview={preview}
                onReload={onReload}
                meta={
                    <>
                        {dims && (
                            <span>
                                {dims.w}×{dims.h}
                            </span>
                        )}
                        <span>{zoomLabel}</span>
                    </>
                }
                actions={
                    <>
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
                    </>
                }
            />
            <div className="ed-viewer-stage">
                {failed ? (
                    <ViewerMessage title="couldn't show image" detail={`WebKit can't decode ${preview.mime}`} error />
                ) : (
                    <img
                        src={url}
                        alt={basename(path)}
                        draggable={false}
                        onError={() => setFailed(true)}
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
