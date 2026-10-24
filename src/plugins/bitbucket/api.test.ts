import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PluginStreamHandlers } from "../../plugin-api/backend";

const fake = vi.hoisted(() => ({
    call: vi.fn(),
    stream: vi.fn(),
    invalidate: vi.fn(),
}));

vi.mock("../../api/plugins", async () => (await import("../../plugin-api/fakeTransport")).fakeTransport(fake));
vi.mock("../../state/resources", async (importOriginal) => ({ ...(await importOriginal<object>()), invalidate: fake.invalidate }));

import { errorText } from "../../plugin-api/format";
import { bitbucketApi, bitbucketHostApi, type BitbucketStatus } from "./api";

const repo = { provider: "sikemux.bitbucket", owner: "team", name: "app" };
const refused = { category: "auth", message: "bitbucket: sign-in expired" };
const conflict = { category: "http", message: "bitbucket: http 409", status: 409 };

/** Only the sign-in status is read again: re-reading the answer that just failed would fail again, forever. */
const recheckedSignIn = () =>
    fake.invalidate.mock.calls.some(
        ([matches]) => (matches as (kind: string) => boolean)("host.status") && !(matches as (kind: string) => boolean)("host.pulls"),
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
    it("has the sign-in status read again when a call finds the sign-in refused", async () => {
        fake.call.mockRejectedValue(refused);
        await expect(bitbucketHostApi.pulls(repo, "OPEN")).rejects.toBe(refused);
        expect(recheckedSignIn()).toBe(true);
    });

    it("does so on a 401 and when nothing is set up", async () => {
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
        expect(errorText(refused)).toBe("bitbucket: sign-in expired");
        expect(errorText(new Error("boom"))).toBe("boom");
    });
});

describe("what Bitbucket has no counterpart for", () => {
    it("refuses changes it cannot make without asking Bitbucket", async () => {
        await expect(bitbucketHostApi.deleteRun(repo, "1")).rejects.toThrow("Bitbucket cannot delete a run");
        await expect(bitbucketHostApi.downloadAsset(repo, 1, "a.zip")).rejects.toThrow("Bitbucket cannot download release files");
        expect(fake.call).not.toHaveBeenCalled();
    });

    it("says whether a thread is an issue's, since Bitbucket numbers issues apart from pull requests", async () => {
        fake.call.mockResolvedValue([]);
        await bitbucketHostApi.timeline(repo, 4, "issue");
        await bitbucketHostApi.addComment(repo, 4, "seen", "issue");
        await bitbucketHostApi.comments(repo, 4);
        expect(fake.call.mock.calls).toEqual([
            ["timeline", { ...repo, number: 4, of: "issue" }],
            ["addComment", { ...repo, number: 4, body: "seen", of: "issue" }],
            ["comments", { ...repo, number: 4, of: "pull" }],
        ]);
    });

    it("asks for issues, releases and the inbox", async () => {
        fake.call.mockResolvedValue([]);
        await bitbucketHostApi.issues(repo, "closed", 2);
        await bitbucketHostApi.setIssueState(repo, 4, "closed");
        await bitbucketHostApi.releases(repo);
        await bitbucketHostApi.inbox("ada-id", true);
        await bitbucketHostApi.markRead("ada-id", "team/app#7");
        expect(fake.call.mock.calls).toEqual([
            ["issues", { ...repo, state: "closed", page: 2 }],
            ["setIssueState", { ...repo, number: 4, state: "closed" }],
            ["releases", repo],
            ["inbox", { account: "ada-id", all: true }],
            ["markRead", { account: "ada-id", id: "team/app#7" }],
        ]);
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
        await bitbucketHostApi.rerun(repo, "{uuid}", true);
        await bitbucketHostApi.dispatch(repo, "custom: deploy", "main", { env: "prod" });
        expect(fake.call.mock.calls).toEqual([
            ["rerun", { ...repo, runId: "{uuid}", failedOnly: true }],
            ["dispatch", { ...repo, workflowId: "custom: deploy", gitRef: "main", inputs: { env: "prod" } }],
        ]);
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
        await expect(signIn.done).rejects.toThrow("before it finished");
    });

    it("stops the stream when cancelled", async () => {
        const flow = streaming(() => {});
        const signIn = bitbucketApi.signInWithBrowser(vi.fn());
        signIn.cancel();
        await expect(signIn.done).rejects.toThrow("cancelled");
        expect(flow.stop).toHaveBeenCalled();
    });
});
