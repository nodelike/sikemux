import { useEffect, useState } from "react";
import { fsapi } from "../api/fs";
import { previewUrl } from "../editor/viewers/fileKinds";
import { useStore } from "../state/store";
import { preferencesOf } from "../state/preferences";

const loads = new Map<string, Promise<HTMLImageElement | null>>();
const loaded = new Map<string, HTMLImageElement | null>();

/* The pane dithers the picture in two-pixel dots, so it never shows more of it than this. */
const DOT_PX = 2;

/**
 * The picture at no more than one texel per dot on this screen. Every pane
 * uploads its own copy to the GPU when its screen first comes on stage, and a
 * full-size photo made that upload freeze the swipe.
 */
async function fitToScreen(blob: Blob): Promise<Blob> {
    const bitmap = await createImageBitmap(blob);
    const longest = Math.ceil((Math.max(screen.width, screen.height) * devicePixelRatio) / DOT_PX);
    const scale = longest / Math.max(bitmap.width, bitmap.height);
    if (scale >= 1) {
        bitmap.close();
        return blob;
    }
    const canvas = document.createElement("canvas");
    canvas.width = Math.round(bitmap.width * scale);
    canvas.height = Math.round(bitmap.height * scale);
    const context = canvas.getContext("2d");
    if (!context) {
        bitmap.close();
        return blob;
    }
    context.imageSmoothingQuality = "high";
    context.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    bitmap.close();
    return new Promise((done) => canvas.toBlob((small) => done(small ?? blob), "image/jpeg", 0.92));
}

/*
 * Read through a blob so the picture is same-origin: WebGL refuses to upload
 * one straight off `preview://`.
 */
async function decode(path: string): Promise<HTMLImageElement | null> {
    const { mime } = await fsapi.previewFile(path);
    if (!mime.startsWith("image/")) return null;
    const blob = await fitToScreen(await (await fetch(previewUrl(path))).blob());
    const image = new Image();
    image.src = URL.createObjectURL(blob);
    await image.decode();
    return image;
}

function loadPaneImage(path: string): Promise<HTMLImageElement | null> {
    let load = loads.get(path);
    if (!load) {
        load = decode(path)
            .then((image) => {
                loaded.set(path, image);
                return image;
            })
            .catch((error: unknown) => {
                console.warn("Pane image unavailable:", error instanceof Error ? error.message : error);
                loads.delete(path);
                return null;
            });
        loads.set(path, load);
    }
    return load;
}

/**
 * The picture every pane shares, decoded once, and whether it is still on its
 * way. A pane made after it has loaded gets it on its first render.
 */
export function usePaneImageState(): { image: HTMLImageElement | null; loading: boolean } {
    const path = useStore((s) => preferencesOf(s).paneImage);
    const [image, setImage] = useState<{ path: string; image: HTMLImageElement | null } | null>(() =>
        path && loaded.has(path) ? { path, image: loaded.get(path) ?? null } : null,
    );
    useEffect(() => {
        if (!path) return;
        let live = true;
        void loadPaneImage(path).then((next) => {
            if (live) setImage((was) => (was?.path === path && was.image === next ? was : { path, image: next }));
        });
        return () => {
            live = false;
        };
    }, [path]);
    if (!path) return { image: null, loading: false };
    if (image?.path === path) return { image: image.image, loading: false };
    return loaded.has(path) ? { image: loaded.get(path) ?? null, loading: false } : { image: null, loading: true };
}

/** The picture every pane shares, decoded once. Null until it loads, and when none is set or it cannot be read. */
export function usePaneImage(): HTMLImageElement | null {
    return usePaneImageState().image;
}
