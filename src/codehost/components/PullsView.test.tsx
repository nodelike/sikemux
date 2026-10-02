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

const shell = vi.hoisted(() => ({ openUrl: vi.fn(() => Promise.resolve()) }));
vi.mock("../../plugin-api/host", async (importOriginal) => ({
    ...(await importOriginal<typeof import("../../plugin-api/host")>()),
    openUrl: shell.openUrl,
}));

vi.mock("../../api/git", async (importOriginal) => ({ ...(await importOriginal<typeof import("../../api/git")>()), git: localGit }));

vi.mock("../../git/CommitReview", () => ({
    CommitReview: ({ rev, range }: { rev: string; range?: { base: string; files: string[] } }) => (
        <div data-testid="commit-review">{range ? `${range.base}..${rev} ${range.files.join(" ")}` : rev}</div>
    ),
}));

import { invalidate } from "../../plugin-api/resources";
import { setState } from "../../state/store";
import { acceptDialog, dismissDialog, resetDialogsForTests, useDialogs } from "../../state/dialog";
import { useToasts } from "../../state/toast";
import type { CodeHost } from "../registry";
import { resetView, showItem, useHostViews, viewOf } from "../state";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";
import { PullsView, reviewVerdict } from "./PullsView";

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
    expect(toggle).toHaveTextContent("feat: second");

    await user.click(toggle);
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

describe("reviewVerdict", () => {
    it("counts only each person's latest approving or blocking review", () => {
        expect(reviewVerdict([])).toBeNull();
        expect(reviewVerdict([{ author: "a", state: "COMMENTED" }])).toBeNull();
        expect(
            reviewVerdict([
                { author: "a", state: "CHANGES_REQUESTED" },
                { author: "a", state: "APPROVED" },
                { author: "a", state: "DISMISSED" },
            ]),
        ).toBe("Approved");
        expect(
            reviewVerdict([
                { author: "a", state: "APPROVED" },
                { author: null, state: "CHANGES_REQUESTED" },
            ]),
        ).toBe("Changes requested");
    });
});

describe("the list", () => {
    beforeEach(() => {
        useHostViews.setState({ views: {} });
    });

    it("says why it could not read the pull requests, and reads again when asked", async () => {
        api.pulls.mockRejectedValueOnce({ category: "http", message: "http 502" });
        render(view(null));
        expect(await screen.findByText("Could not read pull requests")).toBeInTheDocument();
        expect(screen.getByText("http 502")).toBeInTheDocument();
        await userEvent.click(screen.getByRole("button", { name: "Try again" }));
        expect(await screen.findByText("the run page")).toBeInTheDocument();
    });

    it("says when there are none, and switches between open, closed and all", async () => {
        api.pulls.mockResolvedValue([]);
        render(view(null));
        expect(await screen.findByText("No open pull requests.")).toBeInTheDocument();
        await userEvent.click(screen.getByRole("button", { name: "Closed" }));
        expect(viewOf("p-git").pullState).toBe("closed");
    });

    it("offers to open a pull request for the project's branch when it has none", async () => {
        render(view(null, false, "feat/lonely"));
        expect(await screen.findByText("has no pull request yet.", { exact: false })).toBeInTheDocument();
        await userEvent.click(screen.getByRole("button", { name: "Open one" }));
        expect(viewOf("p-git").composing).toBe("pull");
    });

    it("offers nothing for a branch that already has one open", async () => {
        render(view(null, false, "feat/run-page"));
        await screen.findByText("the run page");
        expect(screen.queryByRole("button", { name: "Open one" })).toBeNull();
    });

    it("opens a pull request clicked, and starts a new one from the plus", async () => {
        render(view(null));
        await userEvent.click(await screen.findByText("the run page"));
        expect(viewOf("p-git").item).toBe(31);
        cleanup();
        render(view(null));
        await userEvent.click(screen.getByRole("button", { name: "New pull request" }));
        expect(viewOf("p-git")).toMatchObject({ composing: "pull", item: null });
    });

    it("shows who was asked to review each one", async () => {
        api.pulls.mockResolvedValue([{ ...pull, reviewers: ["grace"], comments: 4, avatars: { grace: "https://example.test/g.png" } }]);
        render(view(null));
        await screen.findByText("the run page");
        await waitFor(() => expect(api.image).toHaveBeenCalledWith("https://example.test/g.png"));
        expect(screen.getAllByTitle("4 comments").length).toBeGreaterThan(0);
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

    it("says it could not read one, with a way back", async () => {
        api.pull.mockRejectedValue({ category: "http", message: "http 404" });
        render(view(31));
        expect(await screen.findByText("Could not read it")).toBeInTheDocument();
        await userEvent.click(screen.getByRole("button", { name: "Back" }));
        expect(viewOf("p-git").item).toBeNull();
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

    it("merges the way picked", async () => {
        detail({ headSha: SHA });
        await userEvent.click(await within(leftColumn()).findByRole("button", { name: "How to merge" }));
        await userEvent.click(await screen.findByRole("option", { name: "Rebase" }));
        await userEvent.click(within(leftColumn()).getByRole("button", { name: "Merge pull request" }));
        acceptDialog((await openDialog()).id);
        await waitFor(() => expect(api.mergePull).toHaveBeenCalledWith(repo, 31, "rebase", SHA));
    });

    it("does not merge when the person thinks better of it", async () => {
        detail();
        await userEvent.click(await within(leftColumn()).findByRole("button", { name: "Merge pull request" }));
        dismissDialog((await openDialog()).id);
        await waitFor(() => expect(useDialogs.getState().dialog).toBeNull());
        expect(api.mergePull).not.toHaveBeenCalled();
    });

    it("says why a merge was refused", async () => {
        api.mergePull.mockRejectedValue({ category: "http", message: "http 409: the branch moved" });
        detail();
        await userEvent.click(await within(leftColumn()).findByRole("button", { name: "Merge pull request" }));
        acceptDialog((await openDialog()).id);
        await waitFor(() => expect(toasts()).toContain("error: Could not merge #31: http 409: the branch moved"));
        expect(api.mergePull).toHaveBeenCalledWith(repo, 31, "squash", "");
    });

    it("tells what stands between it and merged", async () => {
        const cases: [Partial<Pull>, string, string | null][] = [
            [{ mergeState: "dirty" }, "Conflicts with main", "Resolve the conflicts before merging."],
            [{ mergeState: "blocked" }, "Merging is blocked", "GitHub is waiting on the required reviews and checks."],
            [{ mergeState: "behind", base: null }, "Behind the base branch", "Bring the branch up to date before merging."],
            [{ mergeState: "unstable" }, "Ready to merge", null],
            [{ mergeState: null }, "Checking whether it can merge", null],
        ];
        for (const [extra, title, said] of cases) {
            invalidate(() => true);
            detail(extra);
            expect(await within(leftColumn()).findByText(title)).toBeInTheDocument();
            if (said) expect(within(leftColumn()).getByText(said)).toBeInTheDocument();
            cleanup();
        }
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

    it("sums up its reviews", async () => {
        const cases: [unknown[], string][] = [
            [[], "No reviews yet"],
            [[{ author: "a", avatarUrl: null, state: "COMMENTED", body: "", submittedAt: null }], "No approving review yet"],
            [[{ author: "a", avatarUrl: null, state: "APPROVED", body: "", submittedAt: null }], "Approved"],
            [[{ author: "a", avatarUrl: null, state: "CHANGES_REQUESTED", body: "", submittedAt: null }], "Changes requested"],
        ];
        for (const [reviews, said] of cases) {
            invalidate(() => true);
            api.pullReviews.mockResolvedValue(reviews as never);
            detail();
            await waitFor(() => expect(within(leftColumn()).getByText(said)).toBeInTheDocument());
            cleanup();
        }
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

    it("stays open when closing is called off, and says why closing failed", async () => {
        detail();
        await userEvent.click(await within(leftColumn()).findByRole("button", { name: "Close" }));
        dismissDialog((await openDialog()).id);
        await waitFor(() => expect(within(leftColumn()).getByRole("button", { name: "Close" })).toBeEnabled());
        expect(api.setPullState).not.toHaveBeenCalled();

        api.setPullState.mockRejectedValue(new Error("offline"));
        await userEvent.click(within(leftColumn()).getByRole("button", { name: "Close" }));
        acceptDialog((await openDialog()).id);
        await waitFor(() => expect(toasts()).toContain("error: Could not close it: offline"));
    });

    it("reopens a closed one without asking", async () => {
        detail({ state: "closed" });
        expect(await within(leftColumn()).findByText("Closed without merging")).toBeInTheDocument();
        await userEvent.click(within(leftColumn()).getByRole("button", { name: "Reopen" }));
        expect(useDialogs.getState().dialog).toBeNull();
        await waitFor(() => expect(api.setPullState).toHaveBeenCalledWith(repo, 31, "open"));
        await waitFor(() => expect(toasts()).toContain("success: Reopened #31"));
    });

    it("says why reopening failed, and offers no reopening on a host that cannot", async () => {
        api.setPullState.mockRejectedValue(new Error("gone"));
        detail({ state: "closed" });
        await userEvent.click(await within(leftColumn()).findByRole("button", { name: "Reopen" }));
        await waitFor(() => expect(toasts()).toContain("error: Could not reopen it: gone"));
        cleanup();
        invalidate(() => true);
        const final = { ...host, capabilities: { ...host.capabilities, pulls: { ...host.capabilities.pulls, reopen: false } } };
        detail({ state: "closed" }, { on: final });
        await within(leftColumn()).findByText("Closed without merging");
        expect(within(leftColumn()).queryByRole("button", { name: "Reopen" })).toBeNull();
    });

    it("shows who merged it, with their picture, and copies the merge commit", async () => {
        const writeText = vi.fn(() => Promise.resolve());
        Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
        detail({
            state: "merged",
            mergedBy: "grace",
            mergedAt: "2026-01-02T00:00:00Z",
            mergeCommitSha: "abcdef0123456789",
            avatars: { grace: "https://example.test/grace.png" },
        });
        expect(await within(leftColumn()).findByText("Merged into")).toBeInTheDocument();
        expect(within(leftColumn()).getByText("grace")).toBeInTheDocument();
        await waitFor(() => expect(api.image).toHaveBeenCalledWith("https://example.test/grace.png"));
        await userEvent.click(within(leftColumn()).getByRole("button", { name: "abcdef0" }));
        expect(writeText).toHaveBeenCalledWith("abcdef0123456789");
        await waitFor(() => expect(toasts()).toContain("success: Copied abcdef0"));
    });

    it("shows the author's own picture when they merged it themselves", async () => {
        detail({ state: "merged", mergedBy: "someone", avatarUrl: "https://example.test/someone.png", mergeCommitSha: null });
        await within(leftColumn()).findByText("Merged into");
        await waitFor(() => expect(api.image).toHaveBeenCalledWith("https://example.test/someone.png"));
        expect(within(leftColumn()).queryByRole("button", { name: /^[0-9a-f]{7}$/ })).toBeNull();
    });

    it("says the branch is checked out when the project is on it", async () => {
        detail({}, { projectBranch: "feat/run-page" });
        expect(await within(leftColumn()).findByText("Checked out")).toBeInTheDocument();
        expect(within(leftColumn()).queryByRole("button", { name: /Check out/ })).toBeNull();
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

    it("opens the pull request on the host", async () => {
        detail();
        await userEvent.click(await screen.findByRole("button", { name: "Open on Test host" }));
        expect(shell.openUrl).toHaveBeenCalledWith("https://example.test/pull/31");
    });

    it("counts what was said, and what changed", async () => {
        api.timeline.mockResolvedValue([
            {
                kind: "commented",
                id: 1,
                actor: "a",
                avatarUrl: null,
                association: null,
                at: null,
                body: "hi",
                state: null,
                sha: null,
                message: null,
                subject: null,
            },
            {
                kind: "reviewed",
                id: 2,
                actor: "a",
                avatarUrl: null,
                association: null,
                at: null,
                body: " ",
                state: "approved",
                sha: null,
                message: null,
                subject: null,
            },
        ] as never);
        detail({ changedFiles: 12 });
        await waitFor(() => expect(screen.getByRole("tab", { name: /Conversation/ })).toHaveTextContent("Conversation2"));
        expect(screen.getByRole("tab", { name: /Files changed/ })).toHaveTextContent("12");
        api.timeline.mockResolvedValue([]);
    });

    it("does not offer approving to its own author", async () => {
        detail({}, { login: "someone" });
        expect(await screen.findByText("Test host does not let you approve your own pull request.")).toBeInTheDocument();
    });

    it("goes back from a commit to all of the changes", async () => {
        render(view(31));
        await userEvent.click(await screen.findByRole("button", { name: /^Commits/ }));
        await userEvent.click((await screen.findAllByRole("button", { name: /feat: / }))[0]);
        expect(await screen.findByTestId("commit-review")).toBeInTheDocument();
        await userEvent.click(screen.getByRole("button", { name: "All changes" }));
        await waitFor(() => expect(screen.queryByTestId("commit-review")).toBeNull());
    });

    it("cannot open a commit of a repository that is not checked out here", async () => {
        detail({}, { cwd: null });
        await userEvent.click(await screen.findByRole("button", { name: /^Commits/ }));
        await userEvent.click((await screen.findAllByRole("button", { name: /feat: / }))[0]);
        expect(await screen.findByText("This repository is not checked out here, so its commits cannot be opened.")).toBeInTheDocument();
    });

    it("tells each changed file's kind of change", async () => {
        api.pullFiles.mockResolvedValue([
            { path: "a.ts", status: "added", additions: 1000, deletions: 0 },
            { path: "src/b.ts", status: "removed", additions: 0, deletions: 2 },
            { path: "c.ts", status: "renamed", additions: 0, deletions: 0, previousPath: "old.ts" },
            { path: "d.ts", status: "copied", additions: 0, deletions: 0, previousPath: "e.ts" },
            { path: "f.ts", status: "modified", additions: 1, deletions: 1 },
        ]);
        detail();
        await within(leftColumn()).findByTitle("src/b.ts");
        const letters = Array.from(leftColumn().querySelectorAll(".pr-files .git-status")).map((node) => node.textContent);
        expect(letters).toEqual(["A", "D", "R", "C", "M"]);
        expect(leftColumn().querySelector(".pr-files-stat")?.textContent).toBe(`+${(1001).toLocaleString()}−3`);
    });
});

describe("a pull request's commits", () => {
    it("forgets the commit picked once the fold is closed", async () => {
        render(view(31));
        const toggle = await screen.findByRole("button", { name: /^Commits/ });
        await userEvent.click(toggle);
        await userEvent.click((await screen.findAllByRole("button", { name: /feat: / }))[0]);
        expect(await screen.findByTestId("commit-review")).toBeInTheDocument();
        await userEvent.click(toggle);
        await waitFor(() => expect(screen.queryByTestId("commit-review")).toBeNull());
        expect(screen.getByRole("tab", { name: /Files changed/ })).toHaveAttribute("aria-selected", "true");
    });

    it("finds a commit author's picture by name on a host that can", async () => {
        api.pullCommits.mockResolvedValue([
            { sha: "ddddddd4444444", message: "feat: anon", author: null, avatarUrl: null, date: "2026-01-01T10:00:00Z" },
            { sha: "eeeeeee5555555", message: "feat: named", author: "grace", avatarUrl: null, date: "2026-01-01T11:00:00Z" },
        ]);
        const named = { ...host, avatarForLogin: (login: string) => `https://example.test/${login}.png` };
        render(
            <InHost host={named}>
                <PullsView
                    paneId="p-git"
                    repo={repo}
                    listState="open"
                    item={31}
                    composing={false}
                    projectBranch="main"
                    cwd="/repo"
                    login="me"
                    active
                />
            </InHost>,
        );
        await userEvent.click(await screen.findByRole("button", { name: /^Commits/ }));
        expect(await screen.findByRole("button", { name: /feat: anon/ })).toBeInTheDocument();
        await waitFor(() => expect(api.image).toHaveBeenCalledWith("https://example.test/grace.png"));
    });
});

describe("a pull request with little said about it", () => {
    it("calls an unnamed author someone and lists it with no comment count", async () => {
        api.pulls.mockResolvedValue([{ ...pull, author: null, comments: null as never }]);
        render(view(null));
        expect(await screen.findByText(/someone · #31/)).toBeInTheDocument();
        expect(screen.queryByTitle(/comment/)).toBeNull();
    });
});
