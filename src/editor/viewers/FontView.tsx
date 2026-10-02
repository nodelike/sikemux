import { useEffect, useState } from "react";
import { errMessage } from "../../state/toast";
import { ViewerBar, ViewerMessage, type ViewProps } from "./ViewerBar";

const SIZES = [12, 16, 24, 36, 56];
const SAMPLE = "The quick brown fox jumps over the lazy dog";
const GLYPHS = ["ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz", "0123456789 !?&@#%*(){}[]<>/=+-_"];

let faces = 0;

export function FontView({ path, preview, url, onReload }: ViewProps) {
    const [family, setFamily] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        let live = true;
        let face: FontFace | null = null;
        const name = `sikemux-preview-${++faces}`;
        void fetch(url)
            .then((response) => response.arrayBuffer())
            .then((data) => {
                face = new FontFace(name, data);
                return face.load();
            })
            .then((loaded) => {
                if (!live) return;
                document.fonts.add(loaded);
                setFamily(name);
            })
            .catch((cause: unknown) => {
                if (live) setError(errMessage(cause));
            });
        return () => {
            live = false;
            if (face) document.fonts.delete(face);
        };
    }, [url]);

    return (
        <div className="ed-viewer">
            <ViewerBar path={path} preview={preview} onReload={onReload} />
            <div className="ed-viewer-stage ed-viewer-font">
                {error && <ViewerMessage title="couldn't load font" detail={error} error />}
                {!error && !family && <ViewerMessage title="loading font…" />}
                {family && (
                    <div className="ed-viewer-font-sheet" style={{ fontFamily: `"${family}"` }}>
                        {GLYPHS.map((line) => (
                            <p key={line} style={{ fontSize: 22 }}>
                                {line}
                            </p>
                        ))}
                        {SIZES.map((size) => (
                            <p key={size} style={{ fontSize: size }}>
                                {SAMPLE}
                            </p>
                        ))}
                    </div>
                )}
            </div>
        </div>
    );
}
