import { beforeEach, describe, expect, it, vi } from "vitest";

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

import type { RunTick } from "../../plugin-api/codehost";
import { accountOf, entryOf, gitlabHostApi, type GitlabStatus } from "./api";

const repo = { provider: "sikemux.gitlab", owner: "platform/payments", name: "billing-api" };

const status: GitlabStatus = {
    configured: true,
    account: "gitlab.acme.dev#3",
    host: "gitlab.acme.dev",
    login: "ankit",
    displayName: "Ankit Patidar",
    avatarUrl: null,
    canWriteCi: true,
    ok: true,
    authFailed: false,
    message: null,
};

beforeEach(() => {
    for (const mock of Object.values(fake)) mock.mockReset();
});

describe("the GitLab host API", () => {
    it("says whether a thread is a merge request's or an issue's, since GitLab numbers them apart", async () => {
        fake.call.mockResolvedValue([]);
        await gitlabHostApi.timeline(repo, 5, "issue");
        await gitlabHostApi.comments(repo, 5);
        await gitlabHostApi.addComment(repo, 5, "seen", "issue");
        expect(fake.call.mock.calls).toEqual([
            ["timeline", { ...repo, number: 5, of: "issue" }],
            ["comments", { ...repo, number: 5, of: "pull" }],
            ["addComment", { ...repo, number: 5, body: "seen", of: "issue" }],
        ]);
    });

    it("asks for whole patches for the merge request view, and retries one job", async () => {
        fake.call.mockResolvedValue(undefined);
        await gitlabHostApi.pullFiles(repo, 42);
        await gitlabHostApi.rerunJob(repo, "77");
        await gitlabHostApi.rerun(repo, "1834", true);
        expect(fake.call.mock.calls).toEqual([
            ["pullFiles", { ...repo, number: 42, fullPatches: true }],
            ["rerunJob", { ...repo, jobId: "77" }],
            ["rerun", { ...repo, runId: "1834", failedOnly: true }],
        ]);
    });

    it("clears every host view when GitLab turns the token down", async () => {
        const refused = { category: "auth", message: "gitlab: sign-in failed" };
        fake.call.mockRejectedValue(refused);
        await expect(gitlabHostApi.pulls(repo, "open")).rejects.toBe(refused);
        const [matches] = fake.invalidate.mock.calls[0] as [(kind: string) => boolean];
        expect(matches("host.pulls") && !matches("jira.search")).toBe(true);
    });

    it("forgets the sign-in when a watched pipeline finds it gone", async () => {
        fake.openStream.mockImplementation(async (_method: string, _params: unknown, onTick: (tick: RunTick) => void) => {
            onTick({ run: null, jobs: [], error: "signed out", finished: true, fatal: true, signedOut: true });
            return 9;
        });
        const ticks: RunTick[] = [];
        expect(await gitlabHostApi.watchStart(repo, "1834", (tick) => ticks.push(tick))).toBe(9);
        expect(fake.openStream).toHaveBeenCalledWith("watchRun", { ...repo, runId: "1834" }, expect.any(Function));
        expect(ticks).toHaveLength(1);
        expect(fake.invalidate).toHaveBeenCalled();
    });
});

describe("GitLab accounts", () => {
    it("names a company server beside the person, and warns when the token only reads", () => {
        expect(accountOf(status)).toMatchObject({ id: "gitlab.acme.dev#3", host: "gitlab.acme.dev", warning: null });
        expect(accountOf({ ...status, canWriteCi: false }).warning).toMatch(/read_api only/);
        expect(
            entryOf({ id: "a", host: "gitlab.acme.dev", login: "ankit", displayName: "Ankit Patidar", avatarUrl: null, isDefault: true }).detail,
        ).toBe("Ankit Patidar · gitlab.acme.dev");
        expect(entryOf({ id: "b", host: "gitlab.com", login: "ankit", displayName: "ankit", avatarUrl: null, isDefault: false }).detail).toBeNull();
    });
});
