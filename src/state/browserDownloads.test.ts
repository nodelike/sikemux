import { renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { browserApi, type BrowserDownload } from "../api/browser";
import { fsapi } from "../api/fs";
import { downloadToast, useBrowserDownloads } from "./browserDownloads";
import { useToasts } from "./toast";

vi.mock("../api/browser", async () => {
    const actual = await vi.importActual<typeof import("../api/browser")>("../api/browser");
    return { ...actual, browserApi: { ...actual.browserApi, subscribeDownloads: vi.fn() } };
});
vi.mock("../api/fs", () => ({ fsapi: { revealInFinder: vi.fn().mockResolvedValue(undefined) } }));

function download(state: BrowserDownload["state"]): BrowserDownload {
    return { agentId: "agent-one", tabId: "tab-one", url: "https://a.test/files/report.pdf", path: "/Users/me/Downloads/report (2).pdf", state };
}

afterEach(() => {
    vi.clearAllMocks();
    useToasts.setState({ toasts: [] });
});

describe("browser downloads", () => {
    it("names the file the way it landed on disk", () => {
        expect(downloadToast(download("started")).text).toBe("Downloading report (2).pdf");
        expect(downloadToast(download("failed"))).toMatchObject({ kind: "error", options: { timeoutMs: null } });
    });

    it("names a download by its address when it has no file name yet", () => {
        expect(downloadToast({ ...download("started"), path: "" }).text).toBe("Downloading https://a.test/files/report.pdf");
    });

    it("offers to reveal a finished download", async () => {
        const toast = downloadToast(download("finished"));
        expect(toast.kind).toBe("success");
        await toast.options?.action?.run(1);
        expect(fsapi.revealInFinder).toHaveBeenCalledWith("/Users/me/Downloads/report (2).pdf");
    });

    it("toasts every download report while mounted and stops listening after", async () => {
        let deliver: (download: BrowserDownload) => void = () => {};
        let aborted = false;
        vi.mocked(browserApi.subscribeDownloads).mockImplementation(async (listener, signal) => {
            deliver = listener;
            signal.addEventListener("abort", () => (aborted = true));
            return () => {};
        });
        const { unmount } = renderHook(() => useBrowserDownloads());
        await vi.waitFor(() => expect(browserApi.subscribeDownloads).toHaveBeenCalled());
        deliver(download("started"));
        deliver(download("finished"));
        expect(useToasts.getState().toasts.map((toast) => toast.text)).toEqual(["Downloading report (2).pdf", "Downloaded report (2).pdf"]);
        unmount();
        expect(aborted).toBe(true);
    });
});
