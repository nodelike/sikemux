import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Issue, IssuePage } from "../api";

const api = vi.hoisted(() => ({
    issues: vi.fn(),
    issue: vi.fn(),
    timeline: vi.fn(() => Promise.resolve([])),
}));

const work = vi.hoisted(() => ({ workOnIssue: vi.fn(async () => {}) }));
vi.mock("../workOnIssue", () => work);

import { invalidate } from "../../plugin-api/resources";
import { resetView, useHostView } from "../state";
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
    api.issues.mockReset().mockResolvedValue(pageOf([makeIssue()]));
    api.issue.mockReset().mockResolvedValue(makeIssue());
});

afterEach(cleanup);

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
