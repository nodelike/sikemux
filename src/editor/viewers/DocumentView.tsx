import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { documentPreviewApi, type DocumentPlacement } from "../../api/documentPreview";
import { useNativeViewsOccluded, useStageMoving } from "../../state/nativeViews";
import { swallow } from "../../state/toast";
import { ViewerBar, type ViewProps } from "./ViewerBar";

function samePlacement(a: DocumentPlacement | null, b: DocumentPlacement) {
    return !!a && a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;
}

/* macOS's Quick Look view draws the document; it is a native view laid over
   this stage, so it steps aside for overlays and while the stage slides. */
export function DocumentView({ path, preview, onReload, visible }: ViewProps & { visible: boolean }) {
    const owner = useId();
    const stageRef = useRef<HTMLDivElement>(null);
    const [placement, setPlacement] = useState<DocumentPlacement | null>(null);
    const occluded = useNativeViewsOccluded();
    const moving = useStageMoving();
    const shown = visible && !occluded && !moving && !!placement;

    useLayoutEffect(() => {
        const stage = stageRef.current;
        if (!stage) return;
        let frame = 0;
        const measure = () => {
            frame = 0;
            const rect = stage.getBoundingClientRect();
            const next = { x: Math.round(rect.left), y: Math.round(rect.top), width: Math.round(rect.width), height: Math.round(rect.height) };
            setPlacement((previous) => (samePlacement(previous, next) ? previous : next));
        };
        const schedule = () => {
            if (!frame) frame = requestAnimationFrame(measure);
        };
        measure();
        const observer = new ResizeObserver(schedule);
        observer.observe(stage);
        window.addEventListener("resize", schedule);
        window.addEventListener("transitionend", schedule, true);
        return () => {
            observer.disconnect();
            if (frame) cancelAnimationFrame(frame);
            window.removeEventListener("resize", schedule);
            window.removeEventListener("transitionend", schedule, true);
        };
    }, []);

    useEffect(() => {
        if (shown) void documentPreviewApi.show(owner, path, placement).catch(swallow("show document preview"));
        else void documentPreviewApi.hide(owner).catch(swallow("hide document preview"));
    }, [owner, path, placement, shown]);

    useEffect(() => () => void documentPreviewApi.hide(owner).catch(swallow("hide document preview")), [owner]);

    return (
        <div className="ed-viewer">
            <ViewerBar path={path} preview={preview} onReload={onReload} />
            <div ref={stageRef} className="ed-viewer-stage ed-viewer-document" />
        </div>
    );
}
