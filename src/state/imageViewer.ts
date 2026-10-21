import { externalValue, useExternalValue } from "../lib/externalValue";

/* A picture in a transcript is a thumbnail of something worth looking at
   properly. One is open at a time, and the viewer that shows it is mounted at
   the app's root so it covers the window rather than the pane it came from. */
export interface ShownImage {
    /** The data URL the transcript was drawing, which for a big file is a shrunk copy of it. */
    readonly src: string;
    /** What to call it when it is saved. */
    readonly name: string;
    /** The file it came from, when it came from one. */
    readonly path?: string;
}

const shown = externalValue<ShownImage | null>(null);

export const shownImage = shown.get;

export const showImage = (image: ShownImage): void => shown.set(image);

export const hideImage = (): void => shown.set(null);

export const useShownImage = (): ShownImage | null => useExternalValue(shown);
