import { useEffect, useState } from "react";
import { errMessage } from "../../state/toast";
import { formatBytes, ViewerBar, ViewerMessage, type ViewProps } from "./ViewerBar";

const SHOWN_BYTES = 64 * 1024;

export function hexDump(bytes: Uint8Array): string {
    const hex = (row: Uint8Array) => Array.from(row, (byte) => byte.toString(16).padStart(2, "0")).join(" ");
    const lines: string[] = [];
    for (let at = 0; at < bytes.length; at += 16) {
        const row = bytes.subarray(at, at + 16);
        const columns = `${hex(row.subarray(0, 8))}  ${hex(row.subarray(8))}`.padEnd(48, " ");
        const text = Array.from(row, (byte) => (byte >= 0x20 && byte < 0x7f ? String.fromCharCode(byte) : ".")).join("");
        lines.push(`${at.toString(16).padStart(8, "0")}  ${columns}  ${text}`);
    }
    return lines.join("\n");
}

export function HexView({ path, preview, url, onReload }: ViewProps) {
    const [dump, setDump] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        let live = true;
        void fetch(url, { headers: { Range: `bytes=0-${SHOWN_BYTES - 1}` } })
            .then((response) => response.arrayBuffer())
            .then((data) => {
                if (live) setDump(hexDump(new Uint8Array(data, 0, Math.min(data.byteLength, SHOWN_BYTES))));
            })
            .catch((cause: unknown) => {
                if (live) setError(errMessage(cause));
            });
        return () => {
            live = false;
        };
    }, [url]);

    const truncated = preview.size > SHOWN_BYTES;
    return (
        <div className="ed-viewer">
            <ViewerBar path={path} preview={preview} onReload={onReload} meta={truncated && <span>showing first {formatBytes(SHOWN_BYTES)}</span>} />
            {error ? (
                <div className="ed-viewer-stage">
                    <ViewerMessage title="couldn't read file" detail={error} error />
                </div>
            ) : (
                <pre className="ed-viewer-hex">{dump ?? ""}</pre>
            )}
        </div>
    );
}
