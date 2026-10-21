import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({}) as ReturnType<typeof import("../testing").mockApi>);
vi.mock("../api", async (importOriginal) => {
    const { mockApi } = await import("../testing");
    Object.assign(api, mockApi());
    return { ...(await importOriginal<object>()), gcpApi: api };
});
const notify = vi.hoisted(() => vi.fn());
const pickFile = vi.hoisted(() => vi.fn(() => Promise.resolve("/Users/ankit/Desktop/report.csv")));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), notify, pickFile }));

import { answerEmpty } from "../testing";
import { crumbs, StoragePage } from "./StoragePage";
import { gcp } from "../resources";

const target = { account: null, project: "acme-prod" };

let pane = 0;
const renderPage = () => render(<StoragePage paneId={`gcs-${++pane}`} active target={target} />);

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    answerEmpty(api);
    api.buckets.mockResolvedValue([
        {
            name: "acme-assets",
            location: "US",
            locationType: "multi-region",
            storageClass: "STANDARD",
            publicAccessPrevention: "enforced",
            uniformAccess: true,
            versioning: true,
            createdAt: null,
        },
    ]);
    api.objects.mockImplementation((_target, _bucket, prefix: string) =>
        Promise.resolve(
            prefix === ""
                ? {
                      bucket: "acme-assets",
                      prefix: "",
                      folders: ["logs/"],
                      objects: [{ name: "robots.txt", size: 24, contentType: "text/plain", storageClass: "STANDARD", updatedAt: null }],
                      more: false,
                  }
                : {
                      bucket: "acme-assets",
                      prefix: "logs/",
                      folders: [],
                      objects: [{ name: "logs/app.log", size: 2048, contentType: "text/plain", storageClass: "STANDARD", updatedAt: null }],
                      more: false,
                  },
        ),
    );
    api.objectPreview.mockResolvedValue({
        name: "robots.txt",
        size: 24,
        contentType: "text/plain",
        storageClass: "STANDARD",
        updatedAt: null,
        bucket: "acme-assets",
        preview: "User-agent: *\nDisallow:",
        trimmed: false,
        uri: "gs://acme-assets/robots.txt",
    });
    api.objectDownload.mockResolvedValue({ path: "/Users/ankit/Downloads/robots.txt", size: 24 });
    notify.mockClear();
});

afterEach(() => {
    cleanup();
    gcp.refresh();
});

describe("Cloud Storage", () => {
    it("names a folder's crumbs", () => {
        expect(crumbs("logs/2024/")).toEqual([
            { label: "logs", prefix: "logs/" },
            { label: "2024", prefix: "logs/2024/" },
        ]);
        expect(crumbs("")).toEqual([]);
    });

    it("browses into a folder and back out by its crumbs", async () => {
        renderPage();
        fireEvent.click(await screen.findByRole("row", { name: /acme-assets/ }));
        fireEvent.click(await screen.findByRole("row", { name: /logs\// }));
        await waitFor(() => expect(api.objects).toHaveBeenLastCalledWith(target, "acme-assets", "logs/"));
        expect(await screen.findByText("app.log")).toBeInTheDocument();
        fireEvent.click(within(screen.getByRole("navigation", { name: "Folder" })).getByRole("button", { name: "acme-assets" }));
        expect(await screen.findByText("robots.txt")).toBeInTheDocument();
    });

    it("previews a text object and downloads it", async () => {
        renderPage();
        fireEvent.click(await screen.findByRole("row", { name: /acme-assets/ }));
        fireEvent.click(await screen.findByRole("row", { name: /robots\.txt/ }));
        expect(await screen.findByText(/User-agent: \*/)).toBeInTheDocument();
        expect(screen.getByText("gs://acme-assets/robots.txt")).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Download" }));
        await waitFor(() => expect(notify).toHaveBeenCalledWith("success", "Saved to /Users/ankit/Downloads/robots.txt"));
    });

    it("uploads a picked file into the folder that is open", async () => {
        api.objectUpload.mockResolvedValue({ name: "logs/report.csv", size: 10, contentType: "text/csv", storageClass: "STANDARD", updatedAt: null });
        renderPage();
        fireEvent.click(await screen.findByRole("row", { name: /acme-assets/ }));
        fireEvent.click(await screen.findByRole("row", { name: /logs\// }));
        await screen.findByText("app.log");
        fireEvent.click(screen.getByRole("button", { name: "Upload file" }));
        await waitFor(() => expect(api.objectUpload).toHaveBeenCalledWith(target, "acme-assets", "logs/", "/Users/ankit/Desktop/report.csv"));
        expect(notify).toHaveBeenCalledWith("success", "Uploaded logs/report.csv");
    });
});
