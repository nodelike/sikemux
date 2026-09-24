import { lazy, Suspense, useEffect } from "react";
import { useCorePaletteOpen, usePluginOverlay } from "../../../plugin-api/host";
import { closeRundeckJobPalette, useRundeck } from "../state";

const RundeckJobPalette = lazy(() => import("./RundeckJobPalette").then((module) => ({ default: module.RundeckJobPalette })));

export function RundeckOverlay() {
    const open = useRundeck((state) => state.jobPaletteOpen);
    const corePaletteOpen = useCorePaletteOpen();
    usePluginOverlay(open);
    useEffect(() => {
        if (corePaletteOpen) closeRundeckJobPalette();
    }, [corePaletteOpen]);
    if (!open) return null;
    return (
        <Suspense fallback={null}>
            <RundeckJobPalette />
        </Suspense>
    );
}
