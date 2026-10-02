import { useEffect, useState } from "react";
import { fsapi } from "../api/fs";
import { previewUrl } from "../editor/viewers/fileKinds";
import { useStore } from "../state/store";

const loads = new Map<string, Promise<HTMLImageElement | null>>();

/*
 * Read through a blob so the picture is same-origin: WebGL refuses to upload
 * one straight off `preview://`.
 */
async function decode(path: string): Promise<HTMLImageElement | null> {
    const { mime } = await fsapi.previewFile(path);
    if (!mime.startsWith("image/")) return null;
    const blob = await (await fetch(previewUrl(path))).blob();
    const image = new Image();
    image.src = URL.createObjectURL(blob);
    await image.decode();
    return image;
}

function loadPaneImage(path: string): Promise<HTMLImageElement | null> {
    let load = loads.get(path);
    if (!load) {
        load = decode(path).catch((error: unknown) => {
            console.warn("Pane image unavailable:", error instanceof Error ? error.message : error);
            loads.delete(path);
            return null;
        });
        loads.set(path, load);
    }
    return load;
}

/** The picture every pane shares, decoded once. Null until it loads, and when none is set or it cannot be read. */
export function usePaneImage(): HTMLImageElement | null {
    const path = useStore((s) => s.paneImage);
    const [image, setImage] = useState<{ path: string; image: HTMLImageElement | null } | null>(null);
    useEffect(() => {
        if (!path) return;
        let live = true;
        void loadPaneImage(path).then((loaded) => {
            if (live) setImage({ path, image: loaded });
        });
        return () => {
            live = false;
        };
    }, [path]);
    return path && image?.path === path ? image.image : null;
}
