import type { Theme } from "../themes";

/** A phone draws the picture across its width at most a few times over; past this it only adds bytes. */
const LONGEST_EDGE = 1280;
const QUALITY = 0.82;

export interface BackdropPicture {
    readonly id: string;
    readonly dataUrl: string;
}

function fingerprint(text: string): string {
    let hash = 0x811c9dc5;
    for (let index = 0; index < text.length; index += 1) {
        hash ^= text.charCodeAt(index);
        hash = Math.imul(hash, 0x01000193);
    }
    return (hash >>> 0).toString(16);
}

/** The pane picture shrunk to suit a phone, as a JPEG, named by its content. */
export function backdropPicture(image: HTMLImageElement): BackdropPicture | null {
    const scale = Math.min(1, LONGEST_EDGE / Math.max(image.naturalWidth, image.naturalHeight));
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(image.naturalWidth * scale));
    canvas.height = Math.max(1, Math.round(image.naturalHeight * scale));
    const paint = canvas.getContext("2d");
    if (!paint) return null;
    paint.drawImage(image, 0, 0, canvas.width, canvas.height);
    const dataUrl = canvas.toDataURL("image/jpeg", QUALITY);
    return { id: fingerprint(dataUrl), dataUrl };
}

/** The colour the pane grain draws its dots in, as `shaderField`'s ambient preset picks it. */
export function grainDotColor(theme: Theme): string {
    return theme.dark ? theme.chrome.bgRaised : `color-mix(in srgb, ${theme.chrome.line} 90%, ${theme.chrome.inkMuted})`;
}
