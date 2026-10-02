import { beforeEach, describe, expect, it, vi } from "vitest";

const localGit = vi.hoisted(() => ({
    fetch: vi.fn(),
    branches: vi.fn(),
    checkoutSmart: vi.fn(),
    checkoutRemoteBranch: vi.fn(),
    fetchRef: vi.fn(),
}));
vi.mock("../api/git", async (importOriginal) => ({ ...(await importOriginal<typeof import("../api/git")>()), git: localGit }));

import { subscribe } from "../state/bus";
import { useToasts } from "../state/toast";
import { checkoutPull, isOwnBranch, localBranchOf } from "./checkout";
import type { CodeHost } from "./registry";
import type { Pull } from "./types";

const repo = { provider: "test.host", owner: "nodelike", name: "sikemux" };
const CWD = "/work/sikemux";
const host = { name: "Test host", pullHeadRef: (number: number) => `pull/${number}/head` } as unknown as CodeHost;
const pullOf = (extra: Partial<Pull>) => ({ number: 7, head: "feat/x", headLabel: "nodelike:feat/x", ...extra }) as Pull;

beforeEach(() => {
    for (const mock of Object.values(localGit)) mock.mockReset();
    localGit.fetch.mockResolvedValue(undefined);
    localGit.checkoutSmart.mockResolvedValue("Switched");
    localGit.checkoutRemoteBranch.mockResolvedValue(undefined);
    localGit.fetchRef.mockResolvedValue(undefined);
    useToasts.setState({ toasts: [] });
});

describe("checking out a pull request", () => {
    it("uses the branch itself when it lives on the repository", () => {
        const pull = { number: 7, head: "feat/x", headLabel: "nodelike:feat/x" };
        expect(isOwnBranch(pull, repo)).toBe(true);
        expect(localBranchOf(pull, repo)).toBe("feat/x");
    });

    it("gives a fork's pull request a branch of its own, named after its number", () => {
        const pull = { number: 49, head: "feat/github-actions-plugin", headLabel: "Sujal85526:feat/github-actions-plugin" };
        expect(isOwnBranch(pull, repo)).toBe(false);
        expect(localBranchOf(pull, repo)).toBe("pr-49");
    });

    it("takes a branch whose owner the host did not say as the repository's own", () => {
        expect(isOwnBranch({ head: "feat/x", headLabel: null }, repo)).toBe(true);
        expect(isOwnBranch({ head: null, headLabel: "nodelike:feat/x" }, repo)).toBe(false);
    });

    it("switches to the branch when it is already here, after fetching", async () => {
        localGit.branches.mockResolvedValue([{ name: "main" }, { name: "feat/x" }]);
        const refreshed = vi.fn();
        const stop = subscribe("git-refresh", refreshed);
        await expect(checkoutPull(CWD, host, repo, pullOf({}))).resolves.toBe("Switched");
        stop();
        expect(localGit.fetch).toHaveBeenCalledWith(CWD, "origin");
        expect(localGit.checkoutSmart).toHaveBeenCalledWith(CWD, "feat/x");
        expect(localGit.checkoutRemoteBranch).not.toHaveBeenCalled();
        expect(refreshed).toHaveBeenCalledWith({ type: "git-refresh", repo: CWD });
    });

    it("tracks the remote branch when it is not here yet", async () => {
        localGit.branches.mockResolvedValue([{ name: "main" }]);
        await expect(checkoutPull(CWD, host, repo, pullOf({}))).resolves.toBe("Checked out feat/x");
        expect(localGit.checkoutRemoteBranch).toHaveBeenCalledWith(CWD, "origin", "feat/x", "feat/x");
        expect(localGit.checkoutSmart).not.toHaveBeenCalled();
    });

    it("fetches a fork's pull request by the host's ref onto a branch named after it", async () => {
        const pull = pullOf({ number: 49, headLabel: "someone:feat/x" });
        await checkoutPull(CWD, host, repo, pull);
        expect(localGit.fetchRef).toHaveBeenCalledWith(CWD, "origin", "pull/49/head", "pr-49");
        expect(localGit.checkoutSmart).toHaveBeenCalledWith(CWD, "pr-49");
        expect(localGit.fetch).not.toHaveBeenCalled();
    });

    it("refuses a fork's pull request on a host with no ref to fetch it by", async () => {
        const plain = { name: "Plain host" } as unknown as CodeHost;
        await expect(checkoutPull(CWD, plain, repo, pullOf({ headLabel: "someone:feat/x" }))).rejects.toThrow(
            "Plain host has no way to fetch a pull request from a fork",
        );
        expect(localGit.fetchRef).not.toHaveBeenCalled();
    });

    it("reports a checkout that failed under the pull request's number", async () => {
        localGit.fetch.mockRejectedValue(new Error("no network"));
        await expect(checkoutPull(CWD, host, repo, pullOf({}))).rejects.toThrow("no network");
        expect(useToasts.getState().toasts.map((toast) => toast.text)).toContain("Check out #7: no network");
    });
});
