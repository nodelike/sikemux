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

import { actionsApi, avatarForEmail, avatarForLogin, failureMessage, githubHostApi, type RunTick } from "./api";

const repo = { provider: "sikemux.github", owner: "nodelike", name: "sikemux" };
const refused = { category: "auth", message: "github: sign-in failed: Bad credentials" };
const conflict = { category: "http", message: "github: http 409: the branch moved", status: 409 };

const clearedGithub = () =>
    fake.invalidate.mock.calls.some(
        ([matches]) => (matches as (kind: string) => boolean)("host.pulls") && !(matches as (kind: string) => boolean)("other.kind"),
    );

beforeEach(() => {
    for (const mock of Object.values(fake)) mock.mockReset();
});

describe("a refused token", () => {
    it("clears what the GitHub views remember when a write finds out", async () => {
        fake.call.mockRejectedValue(refused);
        await expect(actionsApi.mergePull(repo, 1, "squash", "a".repeat(40))).rejects.toBe(refused);
        expect(clearedGithub()).toBe(true);
    });

    it("leaves it alone when a write fails for any other reason", async () => {
        fake.call.mockRejectedValue(conflict);
        await expect(actionsApi.addComment(repo, 1, "hi")).rejects.toBe(conflict);
        expect(fake.invalidate).not.toHaveBeenCalled();
    });

    it("clears it when a download is refused", async () => {
        fake.stream.mockImplementation((_method: string, _params: unknown, handlers: PluginStreamHandlers<unknown>) => {
            handlers.onError?.(refused);
            return { stop() {} };
        });
        await expect(actionsApi.downloadArtifact(repo, "3", "build")).rejects.toBe(refused);
        expect(clearedGithub()).toBe(true);
    });

    it("clears it when a watched run finds the account signed out", async () => {
        let onTick: (tick: RunTick) => void = () => {};
        fake.openStream.mockImplementation((_method: string, _params: unknown, deliver: (tick: RunTick) => void) => {
            onTick = deliver;
            return Promise.resolve(1);
        });
        const seen = vi.fn();
        await actionsApi.watchStart(repo, "7", seen);
        onTick({ run: null, jobs: [], error: "github: not signed in", finished: true, fatal: true, signedOut: true });
        expect(seen).toHaveBeenCalledTimes(1);
        expect(clearedGithub()).toBe(true);
    });
});

describe("ids", () => {
    it("names GitHub's numbered runs and jobs by text, and asks GitHub by number", async () => {
        fake.call.mockResolvedValue({
            run: { id: 7, workflowId: 2 },
            jobs: [
                { id: 30, checkRunId: 31 },
                { id: 40, checkRunId: null },
            ],
        });
        const detail = await actionsApi.run(repo, "7");
        expect(fake.call).toHaveBeenCalledWith("run", { ...repo, runId: 7 });
        expect(detail.run).toMatchObject({ id: "7", workflowId: "2" });
        expect(detail.jobs).toMatchObject([
            { id: "30", checkRunId: "31" },
            { id: "40", checkRunId: null },
        ]);
    });
});

describe("avatarForEmail", () => {
    it("reads the account number out of GitHub's private commit email", () => {
        expect(avatarForEmail("145369993+Sujal85526@users.noreply.github.com")).toBe("https://avatars.githubusercontent.com/u/145369993?s=64");
    });

    it("knows nothing about any other address", () => {
        expect(avatarForEmail("someone@example.com")).toBeNull();
        expect(avatarForEmail("Sujal85526@users.noreply.github.com")).toBeNull();
    });
});

describe("failures", () => {
    it("clears the GitHub views when a read finds the token refused by status", async () => {
        fake.call.mockRejectedValue({ category: "http", message: "github: http 401", status: 401 });
        await expect(actionsApi.pulls(repo, "open")).rejects.toMatchObject({ status: 401 });
        expect(clearedGithub()).toBe(true);
    });

    it("clears the GitHub views when the host was never set up", async () => {
        fake.call.mockRejectedValue({ category: "unconfigured", message: "github: not signed in" });
        await expect(actionsApi.issues(repo, "open", 1)).rejects.toBeTruthy();
        expect(clearedGithub()).toBe(true);
    });

    it("drops only the remembered rate limit when GitHub says to slow down", async () => {
        fake.call.mockRejectedValue({ category: "rate-limited", message: "github: rate limited" });
        await expect(actionsApi.releases(repo)).rejects.toBeTruthy();
        expect(fake.invalidate).toHaveBeenCalledTimes(1);
        const [matches] = fake.invalidate.mock.calls[0] as [(kind: string) => boolean];
        expect(matches("host.rateLimit")).toBe(true);
        expect(matches("host.pulls")).toBe(false);
    });

    it("passes a plain error through untouched", async () => {
        const broken = new Error("ipc gone");
        fake.call.mockRejectedValue(broken);
        await expect(actionsApi.branches(repo)).rejects.toBe(broken);
        expect(fake.invalidate).not.toHaveBeenCalled();
    });

    it("reads the message of a plugin failure and of anything else", () => {
        expect(failureMessage(refused)).toBe("github: sign-in failed: Bad credentials");
        expect(failureMessage(new Error("boom"))).toBe("Error: boom");
        expect(failureMessage("plain")).toBe("plain");
    });

    it("does not clear anything when signing in fails", async () => {
        fake.call.mockRejectedValue(refused);
        await expect(actionsApi.signIn("github.com", "bad")).rejects.toBe(refused);
        expect(fake.invalidate).not.toHaveBeenCalled();
    });
});

describe("requests", () => {
    it("asks for runs of one workflow by number, or of every workflow", async () => {
        fake.call.mockResolvedValue({ runs: [{ id: 9, workflowId: 3 }], total: 1 });
        const page = await actionsApi.runs({ ...repo, workflowId: "3", page: 1 } as never);
        expect(fake.call).toHaveBeenLastCalledWith("runs", expect.objectContaining({ workflowId: 3 }));
        expect(page.runs[0]).toMatchObject({ id: "9", workflowId: "3" });

        await actionsApi.runs({ ...repo, page: 1 } as never);
        expect((fake.call.mock.calls[1] as [string, { workflowId?: number }])[1].workflowId).toBeUndefined();
    });

    it("names workflows and artifacts by text", async () => {
        fake.call.mockResolvedValue([{ id: 12, name: "CI" }]);
        await expect(actionsApi.workflows(repo)).resolves.toEqual([{ id: "12", name: "CI" }]);
        await expect(actionsApi.artifacts(repo, "5")).resolves.toEqual([{ id: "12", name: "CI" }]);
        expect(fake.call).toHaveBeenLastCalledWith("artifacts", { ...repo, runId: 5 });
    });

    it("sends the head commit the person saw with a merge", async () => {
        fake.call.mockResolvedValue(undefined);
        await actionsApi.mergePull(repo, 4, "rebase", "abc");
        expect(fake.call).toHaveBeenCalledWith("mergePull", { ...repo, number: 4, method: "rebase", sha: "abc" });
    });

    it("sends a new pull request's fields alongside the repository", async () => {
        fake.call.mockResolvedValue({ number: 10 });
        await actionsApi.createPull(repo, { title: "t", body: "b", head: "feat", base: "main", draft: true });
        expect(fake.call).toHaveBeenCalledWith("createPull", { ...repo, title: "t", body: "b", head: "feat", base: "main", draft: true });
    });

    it("re-runs without debug logging unless asked", async () => {
        fake.call.mockResolvedValue(undefined);
        await actionsApi.rerun(repo, "8", true);
        await actionsApi.rerunJob(repo, "9", true);
        await actionsApi.reviewDeployment(repo, "8", [1], "approved");
        expect(fake.call.mock.calls).toEqual([
            ["rerun", { ...repo, runId: 8, failedOnly: true, debug: false }],
            ["rerunJob", { ...repo, jobId: 9, debug: true }],
            ["reviewDeployment", { ...repo, runId: 8, environmentIds: [1], state: "approved", comment: "" }],
        ]);
    });

    it("lists fifty of the person's repositories unless told otherwise", async () => {
        fake.call.mockResolvedValue([]);
        await actionsApi.myRepos("acc");
        expect(fake.call).toHaveBeenCalledWith("myRepos", { account: "acc", limit: 50 });
    });

    it("reads an earlier attempt of a run with text ids", async () => {
        fake.call.mockResolvedValue({ run: { id: 1, workflowId: 2 }, jobs: [] });
        const detail = await actionsApi.runAttempt(repo, "1", 2);
        expect(fake.call).toHaveBeenCalledWith("runAttempt", { ...repo, runId: 1, attempt: 2 });
        expect(detail.run.id).toBe("1");
    });
});

describe("image", () => {
    it("fetches each picture once", async () => {
        fake.call.mockResolvedValue("data:image/png;base64,AA");
        await actionsApi.image("https://a/once.png");
        await expect(actionsApi.image("https://a/once.png")).resolves.toBe("data:image/png;base64,AA");
        expect(fake.call).toHaveBeenCalledTimes(1);
    });

    it("tries a failed picture again", async () => {
        fake.call.mockRejectedValueOnce(new Error("offline")).mockResolvedValueOnce("data:ok");
        await expect(actionsApi.image("https://a/retry.png")).rejects.toThrow("offline");
        await expect(actionsApi.image("https://a/retry.png")).resolves.toBe("data:ok");
        expect(fake.call).toHaveBeenCalledTimes(2);
    });

    it("forgets the oldest picture once it holds too many", async () => {
        fake.call.mockResolvedValue("data:x");
        await actionsApi.image("https://a/first.png");
        for (let index = 0; index < 400; index += 1) await actionsApi.image(`https://a/fill-${index}.png`);
        fake.call.mockClear();
        await actionsApi.image("https://a/first.png");
        expect(fake.call).toHaveBeenCalledWith("image", { url: "https://a/first.png" });
    });
});

describe("downloads", () => {
    const streamOf = (ticks: unknown[]) =>
        fake.stream.mockImplementation((_method: string, _params: unknown, handlers: PluginStreamHandlers<unknown>) => {
            for (const tick of ticks) handlers.onItem(tick);
            handlers.onEnd?.();
            return { stop() {} };
        });

    it("reports progress and resolves with where the file was saved", async () => {
        streamOf([
            { received: 1, total: 2, saved: null },
            { received: 2, total: 2, saved: null },
            { received: 2, total: 2, saved: { path: "/tmp/build.zip" } },
        ]);
        const progress = vi.fn();
        await expect(actionsApi.downloadAsset(repo, 77, "build.zip", progress)).resolves.toEqual({ path: "/tmp/build.zip" });
        expect(fake.stream).toHaveBeenCalledWith("downloadAsset", { ...repo, assetId: 77, fileName: "build.zip" }, expect.anything());
        expect(progress).toHaveBeenCalledTimes(2);
    });

    it("fails when the stream ends without saving anything", async () => {
        streamOf([{ received: 1, total: 2, saved: null }]);
        await expect(actionsApi.downloadArtifact(repo, "3", "build")).rejects.toThrow("without saving");
    });

    it("keeps the views when a download fails for another reason", async () => {
        fake.stream.mockImplementation((_method: string, _params: unknown, handlers: PluginStreamHandlers<unknown>) => {
            handlers.onError?.(conflict as never);
            return { stop() {} };
        });
        await expect(actionsApi.downloadArtifact(repo, "3", "build")).rejects.toBe(conflict);
        expect(fake.invalidate).not.toHaveBeenCalled();
    });
});

describe("watching a run", () => {
    it("hands over ticks with text ids and keeps the views while signed in", async () => {
        let onTick: (tick: unknown) => void = () => {};
        fake.openStream.mockImplementation((_method: string, _params: unknown, deliver: (tick: unknown) => void) => {
            onTick = deliver;
            return Promise.resolve(3);
        });
        const seen = vi.fn();
        await expect(actionsApi.watchStart(repo, "7", seen)).resolves.toBe(3);
        expect(fake.openStream).toHaveBeenCalledWith("watchRun", { ...repo, runId: 7 }, expect.any(Function));
        onTick({ run: { id: 7, workflowId: 1 }, jobs: [{ id: 5, checkRunId: 6 }], error: null, finished: false, fatal: false, signedOut: false });
        expect(seen.mock.calls[0][0]).toMatchObject({ run: { id: "7" }, jobs: [{ id: "5", checkRunId: "6" }] });
        expect(fake.invalidate).not.toHaveBeenCalled();
    });

    it("clears the views when the watch cannot start for a refused token", async () => {
        fake.openStream.mockRejectedValue(refused);
        await expect(actionsApi.watchStart(repo, "7", vi.fn())).rejects.toBe(refused);
        expect(clearedGithub()).toBe(true);
    });

    it("keeps the views when the watch cannot start for another reason", async () => {
        fake.openStream.mockRejectedValue(conflict);
        await expect(actionsApi.watchStart(repo, "7", vi.fn())).rejects.toBe(conflict);
        expect(fake.invalidate).not.toHaveBeenCalled();
    });
});

describe("githubHostApi", () => {
    const status = {
        configured: true,
        account: "acc-1",
        host: "github.com",
        login: "ada",
        tokenSource: "keychain",
        tokenVariable: null,
        scopes: ["repo"],
        canWriteWorkflows: true,
        ok: true,
        authFailed: false,
        message: null,
    };

    it("reads the account with its avatar and no warning when it can run workflows", async () => {
        fake.call.mockResolvedValue(status);
        await expect(githubHostApi.status("acc-1")).resolves.toEqual({
            id: "acc-1",
            ok: true,
            login: "ada",
            avatarUrl: "https://avatars.githubusercontent.com/ada?s=64",
            host: "github.com",
            canWriteCi: true,
            warning: null,
        });
        expect(fake.call).toHaveBeenCalledWith("status", { account: "acc-1" });
    });

    it("warns when a working token lacks the workflow scope, and shows no avatar without a login", async () => {
        fake.call.mockResolvedValue({ ...status, canWriteWorkflows: false });
        expect((await githubHostApi.status(null)).warning).toMatch(/workflow scope/);

        fake.call.mockResolvedValue({ ...status, ok: false, canWriteWorkflows: false, login: "" });
        const signedOut = await githubHostApi.status(null);
        expect(signedOut.warning).toBeNull();
        expect(signedOut.avatarUrl).toBeNull();
    });

    it("finds avatars on a company's own GitHub once signed in there", async () => {
        fake.call.mockResolvedValue({ ...status, host: "git.corp.example" });
        const account = await githubHostApi.status(null);
        expect(account.avatarUrl).toBe("https://git.corp.example/ada.png?size=64");
        expect(avatarForLogin("grace")).toBe("https://git.corp.example/grace.png?size=64");
        expect(avatarForLogin("dependabot[bot]")).toBeNull();
        expect(avatarForLogin("org/team")).toBeNull();

        fake.call.mockResolvedValue(status);
        await githubHostApi.status(null);
        expect(avatarForLogin("grace")).toBe("https://avatars.githubusercontent.com/grace?s=64");
    });

    it("keeps the last known host when a status names none", async () => {
        fake.call.mockResolvedValue({ ...status, host: "git.corp.example" });
        await githubHostApi.status(null);
        fake.call.mockResolvedValue({ ...status, host: "", login: "" });
        await githubHostApi.status(null);
        expect(avatarForLogin("grace")).toBe("https://git.corp.example/grace.png?size=64");
        fake.call.mockResolvedValue(status);
        await githubHostApi.status(null);
    });

    it("lists accounts, naming the host only when it is not github.com", async () => {
        fake.call.mockResolvedValue([
            { id: "1", host: "github.com", login: "ada", isDefault: true },
            { id: "2", host: "git.corp.example", login: "grace", isDefault: false },
        ]);
        await expect(githubHostApi.accounts()).resolves.toEqual([
            { id: "1", login: "ada", detail: null, avatarUrl: "https://avatars.githubusercontent.com/ada?s=64", isDefault: true },
            { id: "2", login: "grace", detail: "git.corp.example", avatarUrl: "https://git.corp.example/grace.png?size=64", isDefault: false },
        ]);
        expect(fake.call).toHaveBeenCalledWith("accounts");
    });

    it("asks by id when choosing the default account", async () => {
        fake.call.mockResolvedValue(undefined);
        await githubHostApi.setDefaultAccount("2");
        expect(fake.call).toHaveBeenCalledWith("setDefaultAccount", { id: "2" });
    });
});
