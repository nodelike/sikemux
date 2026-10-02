import { renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ status: vi.fn(), pulls: vi.fn() }));

import { invalidate } from "../plugin-api/resources";
import { claimRemote, pickRemote, pullForBranch, pullMerged, pullsByBranch, useBranchPulls, useHostRepo } from "./project";
import type { CodeHost } from "./registry";
import { registerTestHost, TEST_HOST } from "./testHost";
import type { Pull } from "./types";

registerTestHost(api);

describe("pickRemote", () => {
    it("takes origin, which is what people push to", () => {
        expect(
            pickRemote([
                { name: "upstream", url: "git@github.com:nodelike/sikemux.git" },
                { name: "origin", url: "git@github.com:someone/sikemux.git" },
            ]),
        ).toBe("git@github.com:someone/sikemux.git");
    });

    it("falls back to the only remote there is when none is called origin", () => {
        expect(pickRemote([{ name: "upstream", url: "git@github.com:nodelike/sikemux.git" }])).toBe("git@github.com:nodelike/sikemux.git");
    });

    it("has nothing to pick in a repository with no remotes", () => {
        expect(pickRemote([])).toBeNull();
    });
});

describe("pullsByBranch", () => {
    const repo = { provider: "test.host", owner: "nodelike", name: "sikemux" };
    const pull = (number: number, head: string, owner: string) => ({ number, head, headLabel: `${owner}:${head}` }) as Pull;

    it("finds each of the repository's own branches' pull request", () => {
        const found = pullsByBranch([pull(1, "feat/a", "nodelike"), pull(2, "fix/b", "nodelike")], repo);
        expect(found.get("feat/a")?.number).toBe(1);
        expect(found.get("fix/b")?.number).toBe(2);
    });

    it("leaves out a fork's, whose branch only shares a name with one here", () => {
        expect(pullsByBranch([pull(49, "main", "Sujal85526")], repo).has("main")).toBe(false);
    });
});

describe("pullForBranch", () => {
    const repo = { provider: "test.host", owner: "nodelike", name: "sikemux" };
    const pull = (number: number, head: string, state: string, owner = "nodelike") =>
        ({ number, head, state, headLabel: `${owner}:${head}`, mergedAt: null }) as Pull;

    it("prefers the open pull request, then the newest", () => {
        const pulls = [pull(9, "sikemux/fix", "closed"), pull(7, "sikemux/fix", "open"), pull(3, "other", "open")];
        expect(pullForBranch(pulls, repo, "sikemux/fix")?.number).toBe(7);
        expect(pullForBranch(pulls.slice(0, 1), repo, "sikemux/fix")?.number).toBe(9);
        expect(pullForBranch([pull(4, "sikemux/fix", "open", "fork")], repo, "sikemux/fix")).toBeNull();
    });

    it("counts a closed pull request with a merge time as merged", () => {
        expect(pullMerged({ state: "merged", mergedAt: null })).toBe(true);
        expect(pullMerged({ state: "closed", mergedAt: "2026-09-30T10:00:00Z" })).toBe(true);
        expect(pullMerged({ state: "closed", mergedAt: null })).toBe(false);
        expect(pullMerged(null)).toBe(false);
    });
});

describe("claimRemote", () => {
    const host = (id: string, server: string) =>
        ({
            id,
            api: {
                resolveRemote: (url: string) =>
                    Promise.resolve({
                        repo: { host: new URL(url).host, owner: "team", name: "thing" },
                        slug: "team/thing",
                        sameHost: new URL(url).host === server,
                    }),
            },
        }) as unknown as Pick<CodeHost, "id" | "api">;

    it("goes to the host whose server the remote is on", async () => {
        const hosts = [host("github", "github.com"), host("bitbucket", "bitbucket.org")];
        expect(await claimRemote("https://bitbucket.org/team/thing.git", hosts)).toEqual({ provider: "bitbucket", owner: "team", name: "thing" });
    });

    it("leaves a remote no host serves to the local workbench, even when a host can read its address", async () => {
        expect(await claimRemote("https://gitlab.com/team/thing.git", [host("github", "github.com")])).toBeNull();
    });
});

describe("claimRemote when a host cannot read the address", () => {
    it("moves on to the next host", async () => {
        const broken = { id: "broken", api: { resolveRemote: () => Promise.reject(new Error("bad url")) } } as unknown as Pick<
            CodeHost,
            "id" | "api"
        >;
        const noRepo = { id: "empty", api: { resolveRemote: () => Promise.resolve({ repo: null, slug: null, sameHost: true }) } } as unknown as Pick<
            CodeHost,
            "id" | "api"
        >;
        expect(await claimRemote("ssh://odd", [broken, noRepo])).toBeNull();
    });
});

describe("useHostRepo", () => {
    it("finds nothing, and waits on nothing, without a folder", () => {
        const { result } = renderHook(() => useHostRepo(null, true));
        expect(result.current).toEqual({ repo: null, remote: null, branch: null, loading: false });
    });
});

describe("useBranchPulls", () => {
    const repo = { provider: TEST_HOST, owner: "nodelike", name: "sikemux", account: "ada-id" };
    const open = (number: number, head: string, headLabel: string | null) => ({ number, head, headLabel, state: "open" }) as Pull;

    beforeEach(() => {
        invalidate(() => true);
        api.status.mockReset().mockResolvedValue({ ok: true });
        api.pulls.mockReset().mockResolvedValue([open(1, "feat/a", null), open(2, "main", "fork:main")]);
    });

    it("maps the repository's own branches to their open pull request once signed in", async () => {
        const { result } = renderHook(() => useBranchPulls(repo, true));
        await waitFor(() => expect(result.current.get("feat/a")?.number).toBe(1));
        expect(result.current.has("main")).toBe(false);
        expect(api.status).toHaveBeenCalledWith("ada-id");
        expect(api.pulls).toHaveBeenCalledWith(repo, "open");
    });

    it("asks nothing of a host nobody is signed in to", async () => {
        api.status.mockResolvedValue({ ok: false });
        const { result } = renderHook(() => useBranchPulls({ ...repo, account: "out-id" }, true));
        await waitFor(() => expect(api.status).toHaveBeenCalledWith("out-id"));
        expect(api.pulls).not.toHaveBeenCalled();
        expect(result.current.size).toBe(0);
    });

    it("has nothing without a repository", () => {
        const { result } = renderHook(() => useBranchPulls(null, true));
        expect(result.current.size).toBe(0);
        expect(api.status).not.toHaveBeenCalled();
    });
});
