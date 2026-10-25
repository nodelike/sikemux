import { useEffect } from "react";
import { browserApi, type BrowserDownload } from "../api/browser";
import { fsapi } from "../api/fs";
import { basename, joinPath } from "@shared/lib/paths";
import { notify, reportError, swallow, type ToastKind, type ToastOptions } from "./toast";

function downloadToast(download: BrowserDownload): { kind: ToastKind; text: string; options?: ToastOptions } {
    const name = basename(download.path) || download.url;
    if (download.state === "started") return { kind: "info", text: `Downloading ${name}` };
    if (download.state === "failed") return { kind: "error", text: `Download of ${name} failed`, options: { timeoutMs: null } };
    return {
        kind: "success",
        text: `Downloaded ${name}`,
        options: {
            timeoutMs: 30_000,
            action: {
                label: "Keep in Downloads",
                dismissOnClick: true,
                run: () => keepDownload(download.path).catch(reportError("keep download")),
            },
        },
    };
}

/** Moves a file a tab saved into Downloads beside what is there, keeping its quarantine, and shows it. */
async function keepDownload(held: string): Promise<void> {
    const folder = await fsapi.downloadsDir();
    const name = basename(held);
    const dot = name.lastIndexOf(".");
    const [stem, extension] = dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ""];
    const candidates = Array.from({ length: 100 }, (_, n) => joinPath(folder, n === 0 ? name : `${stem} (${n + 1})${extension}`));
    const kinds = await fsapi.pathKinds(candidates);
    const kept = candidates[kinds.indexOf(null)];
    if (!kept) throw new Error(`Downloads already has 100 files named like ${name}`);
    await fsapi.rename(held, kept);
    await fsapi.revealInFinder(kept);
}

/* A tab's downloads wait in a folder of Sikemux's own until the person keeps
   them, and the toast is the only sign one happened, so every tab reports here. */
export function useBrowserDownloads(): void {
    useEffect(() => {
        const controller = new AbortController();
        void browserApi
            .subscribeDownloads((download) => {
                const toast = downloadToast(download);
                notify(toast.kind, toast.text, toast.options);
            }, controller.signal)
            .catch(swallow("follow browser downloads"));
        return () => controller.abort();
    }, []);
}
