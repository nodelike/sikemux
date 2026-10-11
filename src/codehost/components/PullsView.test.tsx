import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Pull, Run } from "../api";

const api = vi.hoisted(() => ({
    pulls: vi.fn(),
    pull: vi.fn(),
    pullReviews: vi.fn(() => Promise.resolve([])),
    timeline: vi.fn(() => Promise.resolve([])),
    pullCommits: vi.fn(),
    pullFiles: vi.fn(),
    runs: vi.fn(() => Promise.resolve({ runs: [] as Run[], total: 0 })),
    branches: vi.fn(() => Promise.resolve(["main", "feat/run-page"])),
    createPull: vi.fn(() => Promise.resolve({ number: 32 })),
    mergePull: vi.fn(),
    setPullState: vi.fn(),
    reviewPull: vi.fn(),
    image: vi.fn(() => new Promise<string>(() => {})),
}));

const localGit = vi.hoisted(() => ({
    compare: vi.fn(),
    push: vi.fn(),
    fetch: vi.fn(),
    branches: vi.fn(),
    checkoutSmart: vi.fn(),
    checkoutRemoteBranch: vi.fn(),
    fetchRef: vi.fn(),
}));

vi.mock("../../api/git", async (importOriginal) => ({ ...(await importOriginal<typeof import("../../api/git")>()), git: localGit }));

vi.mock("../../git/CommitReview", () => ({
    CommitReview: ({ rev, range }: { rev: string; range?: { base: string; files: string[] } }) => (
        <div data-testid="commit-review">{range ? `${range.base}..${rev} ${range.files.join(" ")}` : rev}</div>
    ),
}));

import { invalidate } from "../../plugin-api/resources";
import { setState } from "../../state/store";
import { acceptDialog, resetDialogsForTests, useDialogs } from "../../state/dialog";
import { useToasts } from "../../state/toast";
import type { CodeHost } from "../registry";
import { resetView, showItem, useHostViews } from "../state";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";
import { PullsView } from "./PullsView";

const host = registerTestHost(api);
const repo = { provider: TEST_HOST, owner: "nodelike", name: "sikemux" };

const pull: Pull = {
    number: 31,
    title: "the run page",
    body: "Brings the run page up to what GitHub shows.",
    state: "open",
    draft: false,
    author: "someone",
    avatarUrl: null,
    authorAssociation: null,
    head: "feat/run-page",
    headLabel: null,
    base: "main",
    headSha: null,
    createdAt: "2026-01-01T12:00:00Z",
    updatedAt: "2026-01-01T12:00:00Z",
    comments: 0,
    additions: 3,
    deletions: 0,
    changedFiles: 1,
    mergeable: true,
    mergeState: "clean",
    labels: [],
    reviewers: [],
    assignees: [],
    milestone: null,
    commits: 2,
    mergedAt: null,
    mergedBy: null,
    mergeCommitSha: null,
    avatars: {},
    url: "https://example.test/pull/31",
};

beforeEach(() => {
    invalidate(() => true);
    resetView("p-git");
    setState({ gitViews: {} });
    api.pulls.mockResolvedValue([pull]);
    api.pull.mockResolvedValue(pull);
    api.pullCommits.mockResolvedValue([
        { sha: "aaaaaaa1111111", message: "feat: first", author: "someone", avatarUrl: null, date: "2026-01-01T10:00:00Z" },
        { sha: "bbbbbbb2222222", message: "feat: second\n\nwith a body", author: "someone", avatarUrl: null, date: "2026-01-01T11:00:00Z" },
    ]);
    api.pullFiles.mockResolvedValue([{ path: "src/run.ts", status: "added", additions: 3, deletions: 0, patch: "@@ -0,0 +1,1 @@\n+x" }]);
});
afterEach(cleanup);

const view = (item: number | null, composing = false, projectBranch = "main") => (
    <InHost host={host}>
        <PullsView
            paneId="p-git"
            repo={repo}
            listState="open"
            item={item}
            composing={composing}
            projectBranch={projectBranch}
            cwd="/repo"
            login="me"
            active
        />
    </InHost>
);

it("reads a pull request in the git pane's columns: its card and files on the left, the conversation on the right", async () => {
    const user = userEvent.setup();
    render(view(31));
    expect((await screen.findAllByRole("heading", { name: /the run page/ })).length).toBeGreaterThan(0);
    const left = document.querySelector(".git-left") as HTMLElement;
    const right = document.querySelector(".git-right") as HTMLElement;
    expect(within(left).getByRole("button", { name: "Merge pull request" })).toBeInTheDocument();
    expect(await within(right).findByText("Brings the run page up to what GitHub shows.")).toBeInTheDocument();
    expect(within(right).getByRole("tab", { name: /Conversation/ })).toHaveAttribute("aria-selected", "true");

    await user.click(await within(left).findByRole("button", { name: /run\.ts/ }));
    expect(within(right).getByRole("tab", { name: /Files changed/ })).toHaveAttribute("aria-selected", "true");
    expect(await within(right).findByText("1 file")).toBeInTheDocument();
});

it("lists the pull request's commits in the history fold, newest first, and shows one's files on the right", async () => {
    const user = userEvent.setup();
    render(view(31));
    const toggle = await screen.findByRole("button", { name: /^Commits/ });
    await waitFor(() => expect(toggle).toHaveTextContent("2"));

    const rows = await screen.findAllByRole("button", { name: /feat: / });
    expect(rows.map((row) => row.getAttribute("aria-label"))).toEqual(["bbbbbbb feat: second", "aaaaaaa feat: first"]);

    await user.click(rows[1]);
    expect(await screen.findByTestId("commit-review")).toHaveTextContent("aaaaaaa1111111");
});

it("gives the list the whole pane while nothing is open", async () => {
    render(view(null));
    expect(await screen.findByText("the run page")).toBeInTheDocument();
    expect(document.querySelector(".gha-full-page .pr-list")).toBeInTheDocument();
    expect(document.querySelector(".git-right")).not.toBeInTheDocument();
    showItem("p-git", null);
});

it("writes a new pull request beside the branch's diff, with its title taken from its only commit", async () => {
    const user = userEvent.setup();
    localGit.compare.mockResolvedValue({
        merge_base: "base000",
        files: [{ path: "src/run.ts", status: "A" }],
        commits: [
            {
                hash: "ccccccc",
                full_hash: "ccccccc3333333",
                parents: ["base000"],
                author: "me",
                author_email: "me@example.test",
                date: "now",
                subject: "feat: the run page",
                refs: [],
                unpushed: false,
            },
        ],
    });
    render(view(null, true, "feat/run-page"));

    const left = document.querySelector(".git-left") as HTMLElement;
    const right = document.querySelector(".git-right") as HTMLElement;
    expect(await within(left).findByRole("button", { name: /run\.ts/ })).toBeInTheDocument();
    await waitFor(() => expect(within(left).getByLabelText("Title")).toHaveValue("feat: the run page"));
    expect(localGit.compare).toHaveBeenCalledWith("/repo", "main", "feat/run-page");
    expect(within(right).getByTestId("commit-review")).toHaveTextContent("base000..feat/run-page src/run.ts");

    await user.click(within(left).getByRole("button", { name: "Open pull request" }));
    await waitFor(() =>
        expect(api.createPull).toHaveBeenCalledWith(repo, {
            title: "feat: the run page",
            head: "feat/run-page",
            base: "main",
            body: "",
            draft: false,
        }),
    );
});

const SHA = "f".repeat(40);
const toasts = () => useToasts.getState().toasts.map((toast) => `${toast.kind}: ${toast.text}`);
const openDialog = async () => {
    await waitFor(() => expect(useDialogs.getState().dialog).not.toBeNull());
    return useDialogs.getState().dialog!;
};

function detail(extra: Partial<Pull> = {}, { on = host as CodeHost, cwd = "/repo" as string | null, projectBranch = "main", login = "me" } = {}) {
    api.pull.mockResolvedValue({ ...pull, ...extra });
    return render(
        <InHost host={on}>
            <PullsView
                paneId="p-git"
                repo={repo}
                listState="open"
                item={31}
                composing={false}
                projectBranch={projectBranch}
                cwd={cwd}
                login={login}
                active
            />
        </InHost>,
    );
}

const leftColumn = () => document.querySelector(".git-left") as HTMLElement;

describe("the list", () => {
    beforeEach(() => {
        useHostViews.setState({ views: {} });
    });

    it("shows how CI went on each one's head commit", async () => {
        const run = (sha: string, conclusion: string): Run => ({
            id: `${sha}-${conclusion}`,
            name: "CI",
            title: "",
            workflowId: "1",
            path: null,
            runNumber: 1,
            attempt: 1,
            event: "pull_request",
            status: "completed",
            conclusion,
            branch: null,
            sha,
            shortSha: sha,
            actor: null,
            avatarUrl: null,
            createdAt: pull.createdAt,
            startedAt: null,
            updatedAt: pull.updatedAt,
            pullRequests: [],
            url: "",
        });
        api.pulls.mockResolvedValue([
            { ...pull, headSha: "abc" },
            { ...pull, number: 32, title: "untested", headSha: "def" },
        ]);
        api.runs.mockResolvedValue({ runs: [run("abc", "success"), run("abc", "failure"), run("zzz", "success")], total: 3 });
        render(view(null));
        await screen.findByText("the run page");
        await waitFor(() => expect(document.querySelectorAll(".pr-row-ci")[0]).toHaveTextContent("Failed"));
        expect(document.querySelectorAll(".pr-row-ci")[1]).toBeEmptyDOMElement();
    });
});

describe("an open pull request", () => {
    beforeEach(() => {
        resetDialogsForTests();
        useToasts.setState({ toasts: [] });
        api.mergePull.mockReset().mockResolvedValue(undefined);
        api.setPullState.mockReset().mockResolvedValue(undefined);
        api.pullReviews.mockReset().mockResolvedValue([]);
        for (const mock of [localGit.fetch, localGit.checkoutSmart, localGit.checkoutRemoteBranch, localGit.fetchRef])
            mock.mockReset().mockResolvedValue("ok");
        localGit.branches.mockReset().mockResolvedValue([]);
    });

    it("merges only the commit the person saw, once they confirm", async () => {
        detail({ headSha: SHA });
        await userEvent.click(await within(leftColumn()).findByRole("button", { name: "Merge pull request" }));
        const asked = await openDialog();
        expect(asked).toMatchObject({ title: "Merge #31 into main?", body: "the run page", confirmLabel: "Merge" });
        acceptDialog(asked.id);
        await waitFor(() => expect(api.mergePull).toHaveBeenCalledWith(repo, 31, "squash", SHA));
        await waitFor(() => expect(toasts()).toContain("success: Merged #31"));
    });

    it("asks once however many times Merge is pressed", async () => {
        detail({ headSha: SHA });
        const merge = await within(leftColumn()).findByRole("button", { name: "Merge pull request" });
        await userEvent.click(merge);
        await userEvent.click(merge);
        const asked = await openDialog();
        expect(useDialogs.getState().queue).toHaveLength(0);
        expect(merge).toBeDisabled();
        acceptDialog(asked.id);
        await waitFor(() => expect(api.mergePull).toHaveBeenCalledTimes(1));
    });

    it("cannot be merged while it conflicts", async () => {
        detail({ mergeState: "dirty" });
        expect(await within(leftColumn()).findByRole("button", { name: "Merge pull request" })).toBeDisabled();
    });

    it("says nothing about merging cleanly on a host that cannot tell", async () => {
        const quiet = { ...host, capabilities: { ...host.capabilities, pulls: { ...host.capabilities.pulls, mergeability: false } } };
        detail({ mergeState: "blocked" }, { on: quiet });
        expect(await within(leftColumn()).findByRole("button", { name: "Merge pull request" })).toBeEnabled();
        expect(within(leftColumn()).queryByText("Merging is blocked")).toBeNull();
    });

    it("offers no merge on a draft", async () => {
        detail({ draft: true });
        expect(await within(leftColumn()).findByText("This is a draft")).toBeInTheDocument();
        expect(within(leftColumn()).getByText("Mark it ready for review on Test host before merging.")).toBeInTheDocument();
        expect(within(leftColumn()).queryByRole("button", { name: "Merge pull request" })).toBeNull();
    });

    it("closes without merging once the person confirms", async () => {
        detail();
        await userEvent.click(await within(leftColumn()).findByRole("button", { name: "Close" }));
        const asked = await openDialog();
        expect(asked).toMatchObject({ title: "Close #31 without merging?", destructive: true });
        acceptDialog(asked.id);
        await waitFor(() => expect(api.setPullState).toHaveBeenCalledWith(repo, 31, "closed"));
        await waitFor(() => expect(toasts()).toContain("success: Closed #31"));
    });

    it("reopens a closed one without asking", async () => {
        detail({ state: "closed" });
        expect(await within(leftColumn()).findByText("Closed without merging")).toBeInTheDocument();
        await userEvent.click(within(leftColumn()).getByRole("button", { name: "Reopen" }));
        expect(useDialogs.getState().dialog).toBeNull();
        await waitFor(() => expect(api.setPullState).toHaveBeenCalledWith(repo, 31, "open"));
        await waitFor(() => expect(toasts()).toContain("success: Reopened #31"));
    });

    it("checks the branch out in the project", async () => {
        localGit.branches.mockResolvedValue([{ name: "feat/run-page" }]);
        detail();
        await userEvent.click(await within(leftColumn()).findByRole("button", { name: "Check out" }));
        await waitFor(() => expect(localGit.checkoutSmart).toHaveBeenCalledWith("/repo", "feat/run-page"));
        await waitFor(() => expect(toasts()).toContain("success: Checked out #31"));
    });

    it("offers no checkout of a fork's branch on a host that cannot fetch it, nor outside the project", async () => {
        detail({ headLabel: "someone-else:feat/run-page" });
        await within(leftColumn()).findByRole("button", { name: "Merge pull request" });
        expect(within(leftColumn()).queryByRole("button", { name: /Check out/ })).toBeNull();
        cleanup();
        detail({}, { cwd: null });
        await within(leftColumn()).findByRole("button", { name: "Merge pull request" });
        expect(within(leftColumn()).queryByRole("button", { name: /Check out/ })).toBeNull();
    });

    it("does not offer approving to its own author", async () => {
        detail({}, { login: "someone" });
        expect(await screen.findByText("Test host does not let you approve your own pull request.")).toBeInTheDocument();
    });
});
