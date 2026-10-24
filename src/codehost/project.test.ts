import { describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ status: vi.fn(), pulls: vi.fn() }));

import { claimRemote, pickRemote, pullMerged, pullState } from "./project";
import type { CodeHost } from "./registry";
import { registerTestHost } from "./testHost";

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

describe("pullMerged", () => {
    it("counts a closed pull request with a merge time as merged", () => {
        expect(pullMerged({ state: "merged", mergedAt: null })).toBe(true);
        expect(pullMerged({ state: "closed", mergedAt: "2026-09-30T10:00:00Z" })).toBe(true);
        expect(pullMerged({ state: "closed", mergedAt: null })).toBe(false);
        expect(pullMerged(null)).toBe(false);
    });
});

describe("pullState", () => {
    it("reads a closed draft as closed, as the pull list does", () => {
        expect(pullState({ state: "closed", draft: true, mergedAt: null })).toBe("closed");
        expect(pullState({ state: "open", draft: true, mergedAt: null })).toBe("draft");
        expect(pullState({ state: "closed", draft: false, mergedAt: "2026-10-01T00:00:00Z" })).toBe("merged");
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
        expect(await claimRemote("https://bitbucket.org/team/thing.git", hosts)).toEqual({
            provider: "bitbucket",
            owner: "team",
            name: "thing",
            host: "bitbucket.org",
        });
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
