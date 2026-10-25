import { useRef, useState } from "react";
import { basename } from "@shared/lib/paths";
import { ViewerBar, ViewerMessage, type ViewProps } from "./ViewerBar";
import { useImageZoom } from "./useImageZoom";

export function ImageView({ path, preview, url, onReload }: ViewProps) {
    const [dims, setDims] = useState<{ w: number; h: number } | null>(null);
    const [failed, setFailed] = useState(false);
    const stageRef = useRef<HTMLDivElement>(null);
    const imageRef = useRef<HTMLImageElement>(null);
    const { zoom, setZoom, zoomBy } = useImageZoom(stageRef, imageRef, dims);
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
                        <button type="button" onClick={() => zoomBy(1.25)} title="Zoom in">
                            +
                        </button>
                        <button type="button" onClick={() => zoomBy(0.8)} title="Zoom out">
                            −
                        </button>
                    </>
                }
            />
            <div ref={stageRef} className="ed-viewer-stage ed-viewer-image">
                {failed ? (
                    <ViewerMessage title="couldn't show image" detail={`WebKit can't decode ${preview.mime}`} error />
                ) : (
                    <img
                        ref={imageRef}
                        src={url}
                        alt={basename(path)}
                        draggable={false}
                        onError={() => setFailed(true)}
                        onLoad={(e) => setDims({ w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight })}
                    />
                )}
            </div>
        </div>
    );
}
