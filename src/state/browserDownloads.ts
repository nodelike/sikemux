import { useEffect } from "react";
import { browserApi, type BrowserDownload } from "../api/browser";
import { fsapi } from "../api/fs";
import { basename } from "../lib/paths";
import { notify, reportError, type ToastKind, type ToastOptions } from "./toast";

function downloadToast(download: BrowserDownload): { kind: ToastKind; text: string; options?: ToastOptions } {
    const name = basename(download.path) || download.url;
    if (download.state === "started") return { kind: "info", text: `Downloading ${name}` };
    if (download.state === "failed") return { kind: "error", text: `Download of ${name} failed`, options: { timeoutMs: null } };
    return {
        kind: "success",
        text: `Downloaded ${name}`,
        options: {
            timeoutMs: 12_000,
            action: {
                label: "Reveal",
                dismissOnClick: true,
                run: () => fsapi.revealInFinder(download.path).catch(reportError("reveal download")),
            },
        },
    };
}

/* Browser tabs save straight into the download folder; the toast is the only
   sign a download happened, so every tab's downloads report here. */
export function useBrowserDownloads(): void {
    useEffect(() => {
        const controller = new AbortController();
        void browserApi
            .subscribeDownloads((download) => {
                const toast = downloadToast(download);
                notify(toast.kind, toast.text, toast.options);
            }, controller.signal)
            .catch(() => {});
        return () => controller.abort();
    }, []);
}
