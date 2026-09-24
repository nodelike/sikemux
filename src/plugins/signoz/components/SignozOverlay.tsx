import { lazy, Suspense, useEffect } from "react";
import { closeCorePalettes, useCorePaletteOpen, usePluginOverlay } from "../../../plugin-api/host";
import { closePalette, useSignoz } from "../state";

const Palette = lazy(() => import("./SignozPalette").then((module) => ({ default: module.Palette })));

export function SignozOverlay() {
    const open = useSignoz((state) => state.paletteOpen);
    const corePaletteOpen = useCorePaletteOpen();
    usePluginOverlay(open);
    useEffect(() => {
        if (corePaletteOpen) closePalette();
    }, [corePaletteOpen]);
    useEffect(() => {
        if (open) closeCorePalettes();
    }, [open]);
    if (!open) return null;
    return (
        <Suspense fallback={null}>
            <Palette />
        </Suspense>
    );
}
