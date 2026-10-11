import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Release, SavedArtifact } from "../api";

const api = vi.hoisted(() => ({ releases: vi.fn(), downloadAsset: vi.fn() }));
const shell = vi.hoisted(() => ({ openUrl: vi.fn(() => Promise.resolve()) }));

vi.mock("../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), openUrl: shell.openUrl }));

import { invalidate } from "../../plugin-api/resources";
import { useToasts } from "../../state/toast";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";
import { ReleasesView } from "./ReleasesView";

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

describe("the release list", () => {
    it("sends the rest to the host once it has read as many as it reads", async () => {
        await renderReleases(Array.from({ length: 100 }, (_, index) => release(100 - index)));
        expect(within(list()).getByText("Newest 100")).toBeTruthy();
        fireEvent.click(screen.getByRole("button", { name: "Older releases on Test host" }));
        expect(shell.openUrl).toHaveBeenCalledWith("https://github.com/nodelike/sikemux/releases");
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
});

describe("a release's files", () => {
    const withAssets = release(1, {
        assets: [
            { id: 11, name: "sikemux_aarch64.dmg", sizeBytes: 10 * 1024 * 1024, downloads: 42 },
            { id: 12, name: "sikemux.tar.gz", sizeBytes: 512, downloads: 3 },
        ],
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
});
