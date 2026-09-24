import { lazy, Suspense, useEffect } from "react";
import { useCorePaletteOpen, usePluginOverlay } from "../../../plugin-api/host";
import { closePalettes, useBruno } from "../state";

const BrunoRequestPalette = lazy(() => import("./BrunoRequestPalette").then((module) => ({ default: module.BrunoRequestPalette })));
const BrunoEnvPalette = lazy(() => import("./BrunoEnvPalette").then((module) => ({ default: module.BrunoEnvPalette })));
const BrunoWorkspacePalette = lazy(() => import("./BrunoWorkspacePalette").then((module) => ({ default: module.BrunoWorkspacePalette })));

export function BrunoOverlay() {
    const requestPalette = useBruno((state) => state.requestPalette);
    const environmentPalette = useBruno((state) => state.environmentPalette);
    const workspacePalette = useBruno((state) => state.workspacePalette);
    const open = requestPalette || environmentPalette || workspacePalette;
    const corePaletteOpen = useCorePaletteOpen();
    usePluginOverlay(open);
    useEffect(() => {
        if (corePaletteOpen) closePalettes();
    }, [corePaletteOpen]);
    if (!open) return null;
    return (
        <Suspense fallback={null}>
            {requestPalette && <BrunoRequestPalette />}
            {environmentPalette && <BrunoEnvPalette />}
            {workspacePalette && <BrunoWorkspacePalette />}
        </Suspense>
    );
}
