import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Issue, IssuePage } from "../api";

const api = vi.hoisted(() => ({
    issues: vi.fn(),
    issue: vi.fn(),
    timeline: vi.fn(() => Promise.resolve([])),
    setIssueState: vi.fn(),
    createIssue: vi.fn(),
}));

const work = vi.hoisted(() => ({ workOnIssue: vi.fn(async () => {}) }));
vi.mock("../workOnIssue", () => work);

import { invalidate } from "../../plugin-api/resources";
import { useToasts } from "../../state/toast";
import { resetView, showItem, updateView, useHostView, viewOf } from "../state";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";
import { IssuesView } from "./IssuesView";

const host = registerTestHost(api);
const repo = { provider: TEST_HOST, owner: "nodelike", name: "sikemux" };

const makeIssue = (overrides: Partial<Issue> = {}): Issue => ({
    number: 5,
    title: "The log jumps",
    body: "It jumps.",
    state: "open",
    stateReason: null,
    author: "someone",
    avatarUrl: null,
    createdAt: "2026-01-01T12:00:00Z",
    updatedAt: "2026-01-01T12:00:00Z",
    closedAt: null,
    comments: 0,
    labels: [],
    assignees: [],
    url: "https://github.com/nodelike/sikemux/issues/5",
    ...overrides,
});

const pageOf = (issues: Issue[], overrides: Partial<IssuePage> = {}): IssuePage => ({ issues, total: issues.length, nextPage: null, ...overrides });

const toasts = () => useToasts.getState().toasts.map((toast) => toast.text);

function Pane({ cwd = null }: { cwd?: string | null }) {
    const view = useHostView("pane");
    return (
        <IssuesView
            paneId="pane"
            repo={repo}
            listState={view.issueState}
            item={view.item}
            composing={view.composing === "issue"}
            page={view.page}
            cwd={cwd}
            active
        />
    );
}

async function renderIssues(cwd: string | null = null) {
    const view = render(
        <InHost host={host}>
            <Pane cwd={cwd} />
        </InHost>,
    );
    await act(async () => {});
    return view;
}

const list = () => document.querySelector(".git-left") as HTMLElement;
const right = () => document.querySelector(".git-right") as HTMLElement;

beforeEach(() => {
    invalidate(() => true);
    resetView("pane");
    useToasts.setState({ toasts: [] });
    api.issues.mockReset().mockResolvedValue(pageOf([makeIssue()]));
    api.issue.mockReset().mockResolvedValue(makeIssue());
    api.setIssueState.mockReset().mockResolvedValue(undefined);
    api.createIssue.mockReset().mockResolvedValue({ number: 9 });
});

afterEach(cleanup);

describe("the issue list", () => {
    it("reads open issues first, with who opened each, who has it and how many comments it has", async () => {
        api.issues.mockResolvedValue(
            pageOf([makeIssue({ comments: 3, assignees: ["alice", "bob"], labels: [{ name: "bug", color: "d73a4a" }] })], { total: 14 }),
        );
        await renderIssues();
        expect(api.issues).toHaveBeenCalledWith(repo, "open", 1);
        const row = within(list()).getByRole("button", { name: /The log jumps/ });
        expect(within(row).getByText("#5")).toBeTruthy();
        expect(within(row).getByText("someone")).toBeTruthy();
        expect(within(row).getByText("→ alice, bob")).toBeTruthy();
        expect(within(row).getByText("bug")).toBeTruthy();
        expect(within(row).getByTitle("3 comments")).toBeTruthy();
        expect(within(list()).getByText("14")).toBeTruthy();
    });

    it("switches between open, closed and every issue", async () => {
        await renderIssues();
        fireEvent.click(screen.getByRole("button", { name: "Closed" }));
        await act(async () => {});
        expect(api.issues).toHaveBeenLastCalledWith(repo, "closed", 1);
        expect(screen.getByRole("button", { name: "Closed" }).dataset.on).toBe("1");
        fireEvent.click(screen.getByRole("button", { name: "All" }));
        await act(async () => {});
        expect(api.issues).toHaveBeenLastCalledWith(repo, "all", 1);
    });

    it("says which issues there are none of", async () => {
        api.issues.mockResolvedValue(pageOf([]));
        await renderIssues();
        expect(screen.getByText("No open issues.")).toBeTruthy();
        fireEvent.click(screen.getByRole("button", { name: "All" }));
        await act(async () => {});
        expect(screen.getByText("No issues.")).toBeTruthy();
    });

    it("shows placeholders while the first page loads", async () => {
        let answer: (page: IssuePage) => void = () => {};
        api.issues.mockReturnValue(new Promise((resolve) => (answer = resolve)));
        await renderIssues();
        expect(screen.getByLabelText("Loading issues")).toBeTruthy();
        await act(async () => answer(pageOf([])));
        expect(screen.queryByLabelText("Loading issues")).toBeNull();
    });

    it("says why the issues could not be read, and reads them again on request", async () => {
        api.issues.mockRejectedValueOnce("Issues are turned off").mockResolvedValue(pageOf([makeIssue()]));
        await renderIssues();
        expect(screen.getByText("Could not read issues")).toBeTruthy();
        expect(screen.getByText("Issues are turned off")).toBeTruthy();
        fireEvent.click(screen.getByRole("button", { name: "Try again" }));
        await act(async () => {});
        expect(screen.getByText("The log jumps")).toBeTruthy();
    });

    it("pages through older issues and back", async () => {
        api.issues.mockResolvedValue(pageOf([makeIssue()], { nextPage: 2 }));
        await renderIssues();
        expect(screen.getByRole("button", { name: "Newer" })).toHaveProperty("disabled", true);
        fireEvent.click(screen.getByRole("button", { name: "Older" }));
        await act(async () => {});
        expect(api.issues).toHaveBeenLastCalledWith(repo, "open", 2);
        expect(screen.getByText("Page 2")).toBeTruthy();

        api.issues.mockResolvedValue(pageOf([makeIssue()]));
        act(() => updateView("pane", { page: 3 }));
        await act(async () => {});
        expect(screen.getByRole("button", { name: "Older" })).toHaveProperty("disabled", true);
        fireEvent.click(screen.getByRole("button", { name: "Newer" }));
        expect(viewOf("pane").page).toBe(2);
    });

    it("has no pager when every issue fits on one page", async () => {
        await renderIssues();
        expect(screen.queryByRole("button", { name: "Older" })).toBeNull();
    });
});

describe("reading an issue", () => {
    it("asks for one to be picked, then reads the one clicked", async () => {
        await renderIssues();
        expect(within(right()).getByText("Pick an issue to read it.")).toBeTruthy();
        fireEvent.click(within(list()).getByRole("button", { name: /The log jumps/ }));
        await act(async () => {});
        expect(api.issue).toHaveBeenCalledWith(repo, 5);
        expect(within(list()).getByRole("button", { name: /The log jumps/ }).dataset.on).toBe("1");
        expect(within(right()).getByRole("heading", { name: "The log jumps" })).toBeTruthy();
        expect(within(right()).getByText("It jumps.")).toBeTruthy();
    });

    it("names who has it", async () => {
        api.issue.mockResolvedValue(makeIssue({ assignees: ["alice"] }));
        showItem("pane", 5);
        await renderIssues();
        expect(within(right()).getByText("→ alice")).toBeTruthy();
    });

    it("closes an open issue", async () => {
        showItem("pane", 5);
        await renderIssues();
        fireEvent.click(screen.getByRole("button", { name: "Close issue" }));
        await act(async () => {});
        expect(api.setIssueState).toHaveBeenCalledWith(repo, 5, "closed");
        expect(toasts()).toContain("Closed #5");
    });

    it("reopens a closed issue", async () => {
        api.issue.mockResolvedValue(makeIssue({ state: "closed", stateReason: "completed" }));
        showItem("pane", 5);
        await renderIssues();
        expect(right().querySelector(".gha-state-word")?.textContent).toBe("Closed");
        fireEvent.click(screen.getByRole("button", { name: "Reopen issue" }));
        await act(async () => {});
        expect(api.setIssueState).toHaveBeenCalledWith(repo, 5, "open");
        expect(toasts()).toContain("Reopened #5");
    });

    it("says an issue was closed as not planned", async () => {
        api.issue.mockResolvedValue(makeIssue({ state: "closed", stateReason: "not_planned" }));
        showItem("pane", 5);
        await renderIssues();
        const word = right().querySelector(".gha-state-word") as HTMLElement;
        expect(word.textContent).toBe("Closed as not planned");
        expect(word.dataset.state).toBe("not_planned");
    });

    it("says why an issue could not be closed or reopened", async () => {
        api.setIssueState.mockRejectedValue(new Error("locked"));
        showItem("pane", 5);
        await renderIssues();
        fireEvent.click(screen.getByRole("button", { name: "Close issue" }));
        await act(async () => {});
        expect(toasts()).toContain("Could not close it: locked");
        api.issue.mockResolvedValue(makeIssue({ state: "closed" }));
        invalidate((kind) => kind === "host.issue");
        await act(async () => {});
        fireEvent.click(screen.getByRole("button", { name: "Reopen issue" }));
        await act(async () => {});
        expect(toasts()).toContain("Could not reopen it: locked");
    });

    it("says why an issue could not be read", async () => {
        api.issue.mockRejectedValue("Not Found");
        showItem("pane", 5);
        await renderIssues();
        expect(within(right()).getByText("Could not read it")).toBeTruthy();
        expect(within(right()).getByText("Not Found")).toBeTruthy();
    });

    it("shows placeholders while an issue loads", async () => {
        let answer: (issue: Issue) => void = () => {};
        api.issue.mockReturnValue(new Promise((resolve) => (answer = resolve)));
        showItem("pane", 5);
        await renderIssues();
        expect(screen.getByLabelText("Loading issue")).toBeTruthy();
        await act(async () => answer(makeIssue()));
        expect(screen.queryByLabelText("Loading issue")).toBeNull();
    });
});

describe("opening an issue", () => {
    it("opens the new issue once it is written", async () => {
        await renderIssues();
        fireEvent.click(screen.getByRole("button", { name: "New issue" }));
        fireEvent.change(screen.getByRole("textbox", { name: "Title" }), { target: { value: "It broke" } });
        fireEvent.click(screen.getByRole("button", { name: "Open issue" }));
        await act(async () => {});
        expect(api.createIssue).toHaveBeenCalledWith(repo, "It broke", "");
        expect(viewOf("pane")).toMatchObject({ item: 9, composing: null });
    });

    it("goes back to the list when cancelled", async () => {
        await renderIssues();
        fireEvent.click(screen.getByRole("button", { name: "New issue" }));
        fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
        expect(viewOf("pane").composing).toBeNull();
        expect(screen.getByText("The log jumps")).toBeTruthy();
    });

    describe("work on this", () => {
        it("starts an agent on an open issue from its row and from its page", async () => {
            await renderIssues("/repo");
            fireEvent.click(within(list()).getByRole("button", { name: "Work on #5" }));
            expect(work.workOnIssue).toHaveBeenCalledWith(repo, 5, "/repo");

            fireEvent.click(within(list()).getByText("The log jumps"));
            await act(async () => {});
            fireEvent.click(within(right()).getByRole("button", { name: /Work on this/ }));
            expect(work.workOnIssue).toHaveBeenCalledTimes(2);
        });

        it("is not offered for a repository that is not the project's own", async () => {
            await renderIssues(null);
            expect(screen.queryByRole("button", { name: "Work on #5" })).not.toBeInTheDocument();
        });

        it("is not offered on a closed issue's row", async () => {
            api.issues.mockResolvedValue(pageOf([makeIssue({ state: "closed" })]));
            await renderIssues("/repo");
            expect(screen.queryByRole("button", { name: "Work on #5" })).not.toBeInTheDocument();
        });
    });
});
