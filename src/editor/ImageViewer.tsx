import { useEffect, useRef, useState } from "react";
import { fsapi } from "../api/fs";
import { readImageSource } from "../chat/imagePreview";
import { useModalFocus } from "../hooks/useModalFocus";
import { hideImage, useShownImage, type ShownImage } from "../state/imageViewer";
import { useOccludeNativeViews } from "../state/nativeViews";
import { errMessage, notify } from "../state/toast";
import { IconClose, IconDownload } from "../ui/Icons";

/** The base64 half of a data URL, which is all a file needs of one. */
function base64Of(src: string): string | null {
    const marker = ";base64,";
    const at = src.indexOf(marker);
    return at < 0 ? null : src.slice(at + marker.length);
}

/**
 * The picture a transcript was showing a thumbnail of, at the size the window
 * allows. Mounted at the app's root, so it covers everything and steps the
 * native browser views aside the way the other overlays do.
 */
export function ImageViewer() {
    const image = useShownImage();
    useOccludeNativeViews(Boolean(image));
    // Closing unmounts the sheet, so nothing an open one was doing is still
    // going when the next picture opens.
    return image ? <ImageSheet image={image} /> : null;
}

/* A thumbnail of a big picture is a shrunk copy of it, so the file is read
   again here, where the picture is meant to be seen at its own size. */
function useFullSize(image: ShownImage): string {
    const [full, setFull] = useState<string | null>(null);
    const path = image.path;
    useEffect(() => {
        setFull(null);
        if (!path) return;
        let live = true;
        void readImageSource(path).then((src) => {
            if (live) setFull(src);
        });
        return () => {
            live = false;
        };
    }, [path]);
    return full ?? image.src;
}

function ImageSheet({ image }: { image: ShownImage }) {
    const sheetRef = useRef<HTMLDivElement>(null);
    const closeRef = useRef<HTMLButtonElement>(null);
    const [saving, setSaving] = useState(false);
    const src = useFullSize(image);
    useModalFocus(sheetRef);

    useEffect(() => closeRef.current?.focus(), []);

    // Taken before the panes behind the scrim can read it as their own.
    useEffect(() => {
        const onKey = (event: KeyboardEvent) => {
            if (event.key !== "Escape") return;
            event.preventDefault();
            event.stopPropagation();
            hideImage();
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, []);

    const save = async () => {
        setSaving(true);
        try {
            const folder = await fsapi.downloadsDir();
            // A picture that is already a file is copied; one that only ever
            // existed in the transcript is written out of what is on screen.
            const data = image.path ? null : base64Of(image.src);
            if (!image.path && !data) throw new Error("this picture has nothing to save");
            const saved = image.path ? await fsapi.copyIntoDir(image.path, folder) : await fsapi.saveBase64IntoDir(folder, image.name, data!);
            notify("success", `Saved ${saved.split("/").pop()} to Downloads`, {
                action: { label: "Show", run: () => void fsapi.revealInFinder(saved) },
            });
        } catch (error) {
            notify("error", `Could not save the picture: ${errMessage(error)}`);
        } finally {
            setSaving(false);
        }
    };

    return (
        <div className="img-scrim" onMouseDown={hideImage}>
            <div
                ref={sheetRef}
                tabIndex={-1}
                className="img-sheet"
                role="dialog"
                aria-modal="true"
                aria-label={image.name}
                onMouseDown={(event) => event.stopPropagation()}>
                <div className="img-bar">
                    <span className="img-name" title={image.path ?? image.name}>
                        {image.name}
                    </span>
                    <button type="button" aria-label="Save to Downloads" title="Save to Downloads" disabled={saving} onClick={() => void save()}>
                        <IconDownload size={14} />
                    </button>
                    <button ref={closeRef} type="button" aria-label="Close" title="Close — Esc" onClick={hideImage}>
                        <IconClose size={14} />
                    </button>
                </div>
                <img className="img-full" src={src} alt={image.name} />
            </div>
        </div>
    );
}
