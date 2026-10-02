import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Release, SavedArtifact } from "../api";

const api = vi.hoisted(() => ({ releases: vi.fn(), downloadAsset: vi.fn() }));
const shell = vi.hoisted(() => ({ openUrl: vi.fn(() => Promise.resolve()) }));

vi.mock("../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), openUrl: shell.openUrl }));

import { invalidate } from "../../plugin-api/resources";
import { useToasts } from "../../state/toast";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";
import { latestOf, ReleasesView } from "./ReleasesView";

const host = registerTestHost(api);
const repo = { provider: TEST_HOST, owner: "nodelike", name: "sikemux" };

const release = (id: number, overrides: Partial<Release> = {}): Release => ({
    id,
    tag: `v1.${id}.0`,
    name: `v1.${id}.0`,
    body: `Notes for ${id}`,
    draft: false,
    prerelease: false,
    publishedAt: "2026-01-01T12:00:00Z",
    author: null,
    assets: [],
    url: `https://github.com/nodelike/sikemux/releases/tag/v1.${id}.0`,
    ...overrides,
});

const toasts = () => useToasts.getState().toasts.map((toast) => toast.text);
const list = () => document.querySelector(".git-left") as HTMLElement;
const right = () => document.querySelector(".git-right") as HTMLElement;

async function renderReleases(releases: Release[]) {
    api.releases.mockResolvedValue(releases);
    const view = render(
        <InHost host={host}>
            <ReleasesView paneId="pane" repo={repo} active />
        </InHost>,
    );
    await act(async () => {});
    return view;
}

beforeEach(() => {
    invalidate(() => true);
    useToasts.setState({ toasts: [] });
    api.releases.mockReset();
    api.downloadAsset.mockReset();
    shell.openUrl.mockClear();
});

afterEach(cleanup);

describe("latestOf", () => {
    it("skips drafts and pre-releases the way GitHub marks the latest", () => {
        expect(latestOf([release(3, { draft: true }), release(2, { prerelease: true }), release(1)])).toBe(1);
        expect(latestOf([release(1, { draft: true })])).toBeNull();
    });
});

describe("the release list", () => {
    it("tags the latest, drafts and pre-releases, and names the tag when it differs from the title", async () => {
        await renderReleases([release(3, { draft: true }), release(2, { prerelease: true, name: "Second", author: "someone" }), release(1)]);
        const rows = within(list()).getAllByRole("button", { name: /v1|Second/ });
        expect(rows.map((row) => [...row.querySelectorAll(".gha-tag")].map((tag) => tag.textContent))).toEqual([
            ["Draft"],
            ["Pre-release"],
            ["Latest"],
        ]);
        expect(within(rows[1]).getByText("v1.2.0")).toBeTruthy();
        expect(within(rows[1]).getByText("someone")).toBeTruthy();
        expect(rows[2].querySelector(".gha-mono")).toBeNull();
        expect(within(list()).getByText("3 releases")).toBeTruthy();
    });

    it("counts one release in the singular", async () => {
        await renderReleases([release(1)]);
        expect(within(list()).getByText("1 release")).toBeTruthy();
    });

    it("sends the rest to the host once it has read as many as it reads", async () => {
        await renderReleases(Array.from({ length: 100 }, (_, index) => release(100 - index)));
        expect(within(list()).getByText("Newest 100")).toBeTruthy();
        fireEvent.click(screen.getByRole("button", { name: "Older releases on Test host" }));
        expect(shell.openUrl).toHaveBeenCalledWith("https://github.com/nodelike/sikemux/releases");
    });

    it("says when there are no releases", async () => {
        await renderReleases([]);
        expect(within(list()).getByText("This repository has no releases.")).toBeTruthy();
        expect(within(right()).getByText("Nothing released yet.")).toBeTruthy();
    });

    it("shows placeholders while it loads", async () => {
        let answer: (releases: Release[]) => void = () => {};
        api.releases.mockReturnValue(new Promise((resolve) => (answer = resolve)));
        render(
            <InHost host={host}>
                <ReleasesView paneId="pane" repo={repo} active />
            </InHost>,
        );
        await act(async () => {});
        expect(screen.getByLabelText("Loading releases")).toBeTruthy();
        await act(async () => answer([]));
        expect(screen.queryByLabelText("Loading releases")).toBeNull();
    });

    it("says why the releases could not be read, and reads them again on request", async () => {
        api.releases.mockRejectedValueOnce("Not Found").mockResolvedValue([release(1)]);
        render(
            <InHost host={host}>
                <ReleasesView paneId="pane" repo={repo} active />
            </InHost>,
        );
        await act(async () => {});
        expect(screen.getByText("Could not read releases")).toBeTruthy();
        fireEvent.click(screen.getByRole("button", { name: "Try again" }));
        await act(async () => {});
        expect(within(list()).getByText("1 release")).toBeTruthy();
    });

    it("reads the releases again when Refresh is pressed", async () => {
        await renderReleases([release(1)]);
        fireEvent.click(screen.getByRole("button", { name: "Refresh releases" }));
        await act(async () => {});
        expect(api.releases).toHaveBeenCalledTimes(2);
    });
});

describe("reading a release", () => {
    it("starts on the latest release rather than a newer draft, and reads the one picked", async () => {
        await renderReleases([release(3, { draft: true }), release(2)]);
        expect(within(right()).getByRole("heading").textContent).toBe("v1.2.0");
        expect(within(right()).getByText("Notes for 2")).toBeTruthy();
        fireEvent.click(within(list()).getAllByRole("button", { name: /v1\.3\.0/ })[0]);
        expect(within(right()).getByRole("heading").textContent).toBe("v1.3.0");
        expect(within(list()).getAllByRole("button", { name: /v1\.3\.0/ })[0].dataset.on).toBe("1");
    });

    it("starts on the newest when nothing counts as latest", async () => {
        await renderReleases([release(2, { prerelease: true }), release(1, { draft: true })]);
        expect(within(right()).getByRole("heading").textContent).toBe("v1.2.0");
    });

    it("says when a release has no notes", async () => {
        await renderReleases([release(1, { body: "  \n" })]);
        expect(within(right()).getByText("This release has no notes.")).toBeTruthy();
    });

    it("opens the release on the host", async () => {
        await renderReleases([release(1)]);
        fireEvent.click(within(right()).getByRole("button", { name: "Open on Test host" }));
        expect(shell.openUrl).toHaveBeenCalledWith("https://github.com/nodelike/sikemux/releases/tag/v1.1.0");
    });
});

describe("a release's files", () => {
    const withAssets = release(1, {
        assets: [
            { id: 11, name: "sikemux_aarch64.dmg", sizeBytes: 10 * 1024 * 1024, downloads: 42 },
            { id: 12, name: "sikemux.tar.gz", sizeBytes: 512, downloads: 3 },
        ],
    });

    it("counts them on their tab and lists their sizes and downloads", async () => {
        await renderReleases([withAssets]);
        const tab = within(right()).getByRole("tab", { name: /Assets/ });
        expect(tab.textContent).toBe("Assets2");
        expect(within(right()).getByRole("tab", { name: "Notes" }).getAttribute("aria-selected")).toBe("true");
        fireEvent.click(tab);
        expect(tab.getAttribute("aria-selected")).toBe("true");
        expect(within(right()).getByText("10 MB")).toBeTruthy();
        expect(within(right()).getByText("42 downloads")).toBeTruthy();
    });

    it("says when a release has no files", async () => {
        await renderReleases([release(1)]);
        fireEvent.click(within(right()).getByRole("tab", { name: "Assets" }));
        expect(within(right()).getByText("This release has no files.")).toBeTruthy();
    });

    it("downloads a file once however often it is pressed, and says where it went", async () => {
        let saved: (value: SavedArtifact) => void = () => {};
        api.downloadAsset.mockReturnValue(new Promise((resolve) => (saved = resolve)));
        await renderReleases([withAssets]);
        fireEvent.click(within(right()).getByRole("tab", { name: /Assets/ }));
        const download = within(right()).getAllByRole("button", { name: "Download" })[0];
        fireEvent.click(download);
        fireEvent.click(download);
        expect(api.downloadAsset).toHaveBeenCalledTimes(1);
        expect(api.downloadAsset).toHaveBeenCalledWith(repo, 11, "sikemux_aarch64.dmg");
        expect(within(right()).getByRole("button", { name: "Saving…" })).toHaveProperty("disabled", true);
        expect(within(right()).getByRole("button", { name: "Download" })).toHaveProperty("disabled", false);
        await act(async () => saved({ path: "/Downloads/sikemux_aarch64.dmg", bytes: 1 }));
        expect(toasts()).toContain("Saved sikemux_aarch64.dmg to /Downloads/sikemux_aarch64.dmg");
        expect(within(right()).getAllByRole("button", { name: "Download" })).toHaveLength(2);
    });

    it("says why a download failed", async () => {
        api.downloadAsset.mockRejectedValue(new Error("disk full"));
        await renderReleases([withAssets]);
        fireEvent.click(within(right()).getByRole("tab", { name: /Assets/ }));
        fireEvent.click(within(right()).getAllByRole("button", { name: "Download" })[1]);
        await act(async () => {});
        expect(toasts()).toContain("Could not download sikemux.tar.gz: disk full");
    });
});
