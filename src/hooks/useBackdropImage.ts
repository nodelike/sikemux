import { useState } from "react";

/**
 * Whether anyone has given the shell a photographic ground.
 *
 * `--backdrop-image` is a customization hook rather than a setting — it is set
 * from CSS on `:root`, never by a theme — so the only way to know is to read
 * the value back, once. Worth asking, because the element that paints it carries
 * a blur and a mask, which is a full-window composited layer for a picture that
 * is `none` in every shipped build.
 */
function backdropImageSet(): boolean {
    const value = getComputedStyle(document.documentElement).getPropertyValue("--backdrop-image").trim();
    return value !== "" && value !== "none";
}

export function useBackdropImage(): boolean {
    const [present] = useState(backdropImageSet);
    return present;
}
