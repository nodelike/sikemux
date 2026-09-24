import { useEffect } from "react";
import { occludeNativeViews } from "../state/nativeViews";

let openOverlays = 0;

export function pluginOverlayOpen(): boolean {
    return openOverlays > 0;
}

/** While a plugin's palette or dialog is open, native views step aside and the app's shortcuts treat it as a modal. */
export function usePluginOverlay(open: boolean): void {
    useEffect(() => {
        if (!open) return;
        openOverlays += 1;
        const release = occludeNativeViews();
        return () => {
            openOverlays -= 1;
            release();
        };
    }, [open]);
}
