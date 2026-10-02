import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PluginStreamHandlers } from "../../plugin-api/backend";

const fake = vi.hoisted(() => ({
    call: vi.fn(),
    stream: vi.fn(),
    openStream: vi.fn(),
    closeStream: vi.fn(),
    invalidate: vi.fn(),
}));

vi.mock("../../plugin-api/backend", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    createPluginBackend: () => fake,
}));
vi.mock("../../plugin-api/resources", () => ({ invalidate: fake.invalidate }));

import { bitbucketApi, bitbucketHostApi, failureMessage, type BitbucketStatus } from "./api";
import type { RunTick } from "../../plugin-api/codehost";

const repo = { provider: "sikemux.bitbucket", owner: "team", name: "app" };
const refused = { category: "auth", message: "bitbucket: sign-in expired" };
const conflict = { category: "http", message: "bitbucket: http 409", status: 409 };

const clearedHost = () =>
    fake.invalidate.mock.calls.some(
        ([matches]) => (matches as (kind: string) => boolean)("host.pulls") && !(matches as (kind: string) => boolean)("x"),
    );

const status: BitbucketStatus = {
    configured: true,
    account: "ada-id",
    method: "oauth",
    login: "ada",
    displayName: "Ada Lovelace",
    avatarUrl: "https://bitbucket.org/ada.png",
    canWriteCi: true,
    ok: true,
    authFailed: false,
    message: null,
    browserSignIn: true,
};

beforeEach(() => {
    for (const mock of Object.values(fake)) mock.mockReset();
});

describe("failures", () => {
    it("clears the host views when a call finds the sign-in refused", async () => {
        fake.call.mockRejectedValue(refused);
        await expect(bitbucketHostApi.pulls(repo, "OPEN")).rejects.toBe(refused);
        expect(clearedHost()).toBe(true);
    });

    it("clears them on a 401 and when nothing is set up", async () => {
        fake.call.mockRejectedValueOnce({ category: "http", message: "401", status: 401 });
        await expect(bitbucketHostApi.comments(repo, 1)).rejects.toBeTruthy();
        fake.call.mockRejectedValueOnce({ category: "unconfigured", message: "no" });
        await expect(bitbucketHostApi.branches(repo)).rejects.toBeTruthy();
        expect(fake.invalidate).toHaveBeenCalledTimes(2);
    });

    it("keeps them when a call fails for any other reason", async () => {
        fake.call.mockRejectedValueOnce(conflict).mockRejectedValueOnce(new Error("ipc"));
        await expect(bitbucketHostApi.mergePull(repo, 1, "squash", "abc")).rejects.toBe(conflict);
        await expect(bitbucketHostApi.addComment(repo, 1, "hi")).rejects.toThrow("ipc");
        expect(fake.invalidate).not.toHaveBeenCalled();
    });

    it("drops only the remembered rate limit when told to slow down", async () => {
        fake.call.mockRejectedValue({ category: "rate-limited", message: "slow" });
        await expect(bitbucketHostApi.runs({ ...repo } as never)).rejects.toBeTruthy();
        const [matches] = fake.invalidate.mock.calls[0] as [(kind: string) => boolean];
        expect(matches("host.rateLimit")).toBe(true);
        expect(matches("host.runs")).toBe(false);
    });

    it("reads the message of a plugin failure and of anything else", () => {
        expect(failureMessage(refused)).toBe("bitbucket: sign-in expired");
        expect(failureMessage(new Error("boom"))).toBe("Error: boom");
    });
});

describe("what Bitbucket has no counterpart for", () => {
    it("refuses changes it cannot make without asking Bitbucket", async () => {
        await expect(bitbucketHostApi.issues(repo, "open", 1)).rejects.toThrow("Bitbucket cannot show issues here; they live in Jira");
        await expect(bitbucketHostApi.deleteRun(repo, "1")).rejects.toThrow("Bitbucket cannot delete a pipeline");
        await expect(bitbucketHostApi.inbox(null, false)).rejects.toThrow("Bitbucket cannot show notifications");
        expect(fake.call).not.toHaveBeenCalled();
    });

    it("reads as empty where the Git pane only shows extras", async () => {
        await expect(bitbucketHostApi.annotations(repo, "1")).resolves.toEqual([]);
        await expect(bitbucketHostApi.artifacts(repo, "1")).resolves.toEqual([]);
        await expect(bitbucketHostApi.pendingApprovals(repo, "1")).resolves.toEqual([]);
        await expect(bitbucketHostApi.jobSummary(repo, "1")).resolves.toBeNull();
    });
});

describe("requests", () => {
    it("names pipelines and steps by their own text ids", async () => {
        fake.call.mockResolvedValue(undefined);
        await bitbucketHostApi.run(repo, "{uuid}");
        await bitbucketHostApi.rerun(repo, "{uuid}", true);
        await bitbucketHostApi.dispatch(repo, "custom: deploy", "main", { env: "prod" });
        expect(fake.call.mock.calls).toEqual([
            ["run", { ...repo, runId: "{uuid}" }],
            ["rerun", { ...repo, runId: "{uuid}", failedOnly: true }],
            ["dispatch", { ...repo, workflowId: "custom: deploy", gitRef: "main", inputs: { env: "prod" } }],
        ]);
    });

    it("lists fifty repositories unless told otherwise", async () => {
        fake.call.mockResolvedValue([]);
        await bitbucketHostApi.myRepos("ada-id");
        expect(fake.call).toHaveBeenCalledWith("myRepos", { account: "ada-id", limit: 50 });
    });

    it("sends a token with the email it belongs to", async () => {
        fake.call.mockResolvedValue(status);
        await bitbucketApi.signInWithToken("tok", "ada@example.com");
        expect(fake.call).toHaveBeenCalledWith("signInWithToken", { token: "tok", email: "ada@example.com" });
    });
});

describe("accounts", () => {
    it("reads the signed-in account with no warning when it can run pipelines", async () => {
        fake.call.mockResolvedValue(status);
        await expect(bitbucketHostApi.status("ada-id")).resolves.toEqual({
            id: "ada-id",
            ok: true,
            login: "ada",
            avatarUrl: "https://bitbucket.org/ada.png",
            host: "bitbucket.org",
            canWriteCi: true,
            warning: null,
        });
    });

    it("warns only when a working sign-in cannot touch pipelines", async () => {
        fake.call.mockResolvedValue({ ...status, canWriteCi: false });
        expect((await bitbucketHostApi.status(null)).warning).toMatch(/cannot start or stop pipelines/);
        fake.call.mockResolvedValue({ ...status, ok: false, canWriteCi: false });
        expect((await bitbucketHostApi.status(null)).warning).toBeNull();
    });

    it("shows a display name beside the login only when it says something new", async () => {
        fake.call.mockResolvedValue([
            { id: "1", login: "ada", displayName: "Ada Lovelace", avatarUrl: null, isDefault: true },
            { id: "2", login: "grace", displayName: "grace", avatarUrl: "a.png", isDefault: false },
            { id: "3", login: "linus", displayName: null, avatarUrl: null, isDefault: false },
        ]);
        const entries = await bitbucketHostApi.accounts();
        expect(entries.map((entry) => entry.detail)).toEqual(["Ada Lovelace", null, null]);
        expect(entries[1]).toMatchObject({ id: "2", avatarUrl: "a.png", isDefault: false });
    });
});

describe("image", () => {
    it("fetches each picture once and tries a failed one again", async () => {
        fake.call.mockResolvedValueOnce("data:a").mockRejectedValueOnce(new Error("offline")).mockResolvedValueOnce("data:b");
        await bitbucketHostApi.image("https://b/one.png");
        await expect(bitbucketHostApi.image("https://b/one.png")).resolves.toBe("data:a");
        await expect(bitbucketHostApi.image("https://b/two.png")).rejects.toThrow("offline");
        await expect(bitbucketHostApi.image("https://b/two.png")).resolves.toBe("data:b");
        expect(fake.call).toHaveBeenCalledTimes(3);
    });

    it("forgets the oldest picture once it holds too many", async () => {
        fake.call.mockResolvedValue("data:x");
        await bitbucketHostApi.image("https://b/first.png");
        for (let index = 0; index < 400; index += 1) await bitbucketHostApi.image(`https://b/fill-${index}.png`);
        fake.call.mockClear();
        await bitbucketHostApi.image("https://b/first.png");
        expect(fake.call).toHaveBeenCalledTimes(1);
    });
});

describe("signing in with the browser", () => {
    const streaming = (drive: (handlers: PluginStreamHandlers<unknown>) => void) => {
        const stop = vi.fn();
        let handlers: PluginStreamHandlers<unknown> | null = null;
        fake.stream.mockImplementation((_method: string, _params: unknown, given: PluginStreamHandlers<unknown>) => {
            handlers = given;
            return { stop };
        });
        return { stop, run: () => drive(handlers!) };
    };

    it("opens the page it is handed and resolves with the account once the browser comes back", async () => {
        const flow = streaming((handlers) => {
            handlers.onItem({ url: "https://bitbucket.org/site/oauth2/authorize" });
            handlers.onItem(status);
            handlers.onEnd?.();
        });
        const openPage = vi.fn();
        const signIn = bitbucketApi.signInWithBrowser(openPage);
        flow.run();
        await expect(signIn.done).resolves.toEqual(status);
        expect(openPage).toHaveBeenCalledWith("https://bitbucket.org/site/oauth2/authorize");
        expect(fake.stream).toHaveBeenCalledWith("signInWithBrowser", {}, expect.anything());
    });

    it("fails when the sign-in ends without an account", async () => {
        const flow = streaming((handlers) => handlers.onEnd?.());
        const signIn = bitbucketApi.signInWithBrowser(vi.fn());
        flow.run();
        await expect(signIn.done).rejects.toThrow("without an account");
    });

    it("fails with the stream's error", async () => {
        const flow = streaming((handlers) => handlers.onError?.(refused as never));
        const signIn = bitbucketApi.signInWithBrowser(vi.fn());
        flow.run();
        await expect(signIn.done).rejects.toBe(refused);
    });

    it("stops the stream when cancelled", async () => {
        const flow = streaming(() => {});
        const signIn = bitbucketApi.signInWithBrowser(vi.fn());
        signIn.cancel();
        await expect(signIn.done).rejects.toThrow("cancelled");
        expect(flow.stop).toHaveBeenCalled();
    });
});

describe("watching a pipeline", () => {
    it("passes ticks through and clears the views once the sign-in lapses", async () => {
        let deliver: (tick: RunTick) => void = () => {};
        fake.openStream.mockImplementation((_method: string, _params: unknown, onItem: (tick: RunTick) => void) => {
            deliver = onItem;
            return Promise.resolve(4);
        });
        const seen = vi.fn();
        await expect(bitbucketHostApi.watchStart(repo, "{p}", seen)).resolves.toBe(4);
        expect(fake.openStream).toHaveBeenCalledWith("watchRun", { ...repo, runId: "{p}" }, expect.any(Function));
        const quiet: RunTick = { run: null, jobs: [], error: null, finished: false, fatal: false, signedOut: false };
        deliver(quiet);
        expect(fake.invalidate).not.toHaveBeenCalled();
        deliver({ ...quiet, signedOut: true, finished: true, fatal: true });
        expect(seen).toHaveBeenCalledTimes(2);
        expect(clearedHost()).toBe(true);
    });

    it("clears the views only when the watch cannot start for a lapsed sign-in", async () => {
        fake.openStream.mockRejectedValueOnce(conflict).mockRejectedValueOnce(refused);
        await expect(bitbucketHostApi.watchStart(repo, "{p}", vi.fn())).rejects.toBe(conflict);
        expect(fake.invalidate).not.toHaveBeenCalled();
        await expect(bitbucketHostApi.watchStart(repo, "{p}", vi.fn())).rejects.toBe(refused);
        expect(clearedHost()).toBe(true);
    });
});
