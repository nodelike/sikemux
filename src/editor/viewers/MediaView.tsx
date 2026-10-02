import { useEffect, useRef, useState } from "react";
import { ViewerBar, ViewerMessage, type ViewProps } from "./ViewerBar";

function formatDuration(seconds: number): string {
    const whole = Math.round(seconds);
    const h = Math.floor(whole / 3600);
    const m = Math.floor((whole % 3600) / 60);
    const s = String(whole % 60).padStart(2, "0");
    return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

export function MediaView({ kind, visible, path, preview, url, onReload }: ViewProps & { kind: "video" | "audio"; visible: boolean }) {
    const mediaRef = useRef<HTMLVideoElement & HTMLAudioElement>(null);
    const [meta, setMeta] = useState<{ duration: number; w: number; h: number } | null>(null);
    const [failed, setFailed] = useState(false);

    // Taking the source away is what lets WebKit drop the decoder right away.
    useEffect(() => {
        const media = mediaRef.current;
        return () => {
            if (!media) return;
            media.pause();
            media.removeAttribute("src");
            media.load();
        };
    }, []);

    useEffect(() => {
        if (!visible) mediaRef.current?.pause();
    }, [visible]);

    const onLoadedMetadata = () => {
        const media = mediaRef.current;
        if (media) setMeta({ duration: media.duration, w: media.videoWidth ?? 0, h: media.videoHeight ?? 0 });
    };
    const shared = {
        ref: mediaRef,
        src: url,
        controls: true,
        preload: "metadata",
        onLoadedMetadata,
        onError: () => setFailed(true),
    } as const;

    return (
        <div className="ed-viewer">
            <ViewerBar
                path={path}
                preview={preview}
                onReload={onReload}
                meta={
                    meta && (
                        <>
                            {meta.w > 0 && (
                                <span>
                                    {meta.w}×{meta.h}
                                </span>
                            )}
                            {Number.isFinite(meta.duration) && <span>{formatDuration(meta.duration)}</span>}
                        </>
                    )
                }
            />
            <div className="ed-viewer-stage">
                {failed ? (
                    <ViewerMessage title={`couldn't play ${kind}`} detail={`WebKit can't decode this ${preview.mime} file; try open`} error />
                ) : kind === "video" ? (
                    <video {...shared} className="ed-viewer-video" />
                ) : (
                    <audio {...shared} className="ed-viewer-audio" />
                )}
            </div>
        </div>
    );
}
