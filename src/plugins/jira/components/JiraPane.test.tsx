import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { JiraIssue, JiraStatus } from "../api";

const api = vi.hoisted(() => ({
    status: vi.fn(),
    search: vi.fn(),
    issue: vi.fn(),
    filters: vi.fn(),
    transition: vi.fn(),
    assign: vi.fn(),
    setTask: vi.fn(),
    projects: vi.fn(),
    boards: vi.fn(),
    board: vi.fn(),
    moveIssue: vi.fn(),
    signIn: vi.fn(),
}));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), jiraApi: api }));

import { JiraPane } from "./JiraPane";
import { invalidate } from "../../../plugin-api/resources";

const signedIn: JiraStatus = {
    configured: true,
    sites: [{ host: "acme.atlassian.net", displayName: "Me", default: true }],
    ok: true,
    authFailed: false,
    message: null,
    browserSignIn: false,
};

const summary = {
    key: "ABC-12",
    summary: "Fix the login race",
    status: "In Progress",
    statusCategory: "indeterminate" as const,
    priority: "High",
    assignee: { accountId: "a1", name: "Ana" },
    issueType: "Bug",
    sprint: "Sprint 5",
    updated: null,
    url: "https://acme.atlassian.net/browse/ABC-12",
};

const detail: JiraIssue = {
    ...summary,
    project: "ABC",
    reporter: { accountId: "r1", name: "Raj" },
    labels: ["auth"],
    created: null,
    description: "Steps: **open** the app",
    comments: [{ id: "1", author: "Raj", created: "2026-10-04T09:00:00.000+0000", body: "Seen on staging" }],
    commentCount: 1,
    transitions: [{ id: "31", name: "Review", to: "In Review" }],
};

afterEach(() => {
    cleanup();
    invalidate((kind) => kind.startsWith("jira."));
});

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    api.status.mockResolvedValue(signedIn);
    api.search.mockResolvedValue({ issues: [summary], next: null });
    api.filters.mockResolvedValue([{ id: "9", name: "Team bugs", jql: "type = Bug" }]);
    api.issue.mockResolvedValue(detail);
    api.transition.mockResolvedValue({ status: "In Review", transitions: [] });
    api.projects.mockResolvedValue([
        { key: "ABC", name: "Alphabet" },
        { key: "OPS", name: "Operations" },
    ]);
    api.boards.mockResolvedValue([{ id: 7, name: "ABC board", kind: "scrum", project: "ABC" }]);
    api.board.mockResolvedValue({
        id: 7,
        name: "ABC board",
        kind: "scrum",
        sprint: { id: 42, name: "Sprint 5", end: null, goal: "Ship the login fix" },
        columns: [
            { name: "To Do", statusIds: ["1"], issues: [] },
            { name: "In Progress", statusIds: ["3"], issues: [summary] },
            { name: "Done", statusIds: ["4"], issues: [] },
        ],
        truncated: false,
    });
    api.moveIssue.mockResolvedValue(undefined);
});

describe("JiraPane", () => {
    it("ticks a task in the description in Jira, and says so when Jira refuses", async () => {
        api.issue.mockResolvedValue({ ...detail, description: "Acceptance:\n\n- [ ] Sends when **long**\n- [x] Keeps the 400" });
        api.setTask.mockResolvedValue(undefined);
        render(<JiraPane paneId="jira-tasks" active />);
        fireEvent.click(await screen.findByRole("listitem"));
        const [first, second] = await screen.findAllByRole("checkbox");
        expect(first).not.toBeChecked();
        expect(second).toBeChecked();

        await act(async () => fireEvent.click(first));
        expect(api.setTask).toHaveBeenCalledWith("ABC-12", 0, "Sends when long", true, "acme.atlassian.net");

        api.setTask.mockRejectedValue({ category: "not-found", message: "ABC-12 changed in Jira since it was opened" });
        await act(async () => fireEvent.click(second));
        expect(api.setTask).toHaveBeenLastCalledWith("ABC-12", 1, "Keeps the 400", false, "acme.atlassian.net");
        expect(second).toBeChecked();
    });

    it("puts a card back and says why when Jira will not move it", async () => {
        api.moveIssue.mockRejectedValue({ category: "bad-params", message: "ABC-12's workflow has no way into Done from where it is now" });
        render(<JiraPane paneId="jira-board-refused" active />);
        fireEvent.click(await screen.findByRole("button", { name: "ABC board" }));
        fireEvent.contextMenu(await screen.findByRole("listitem"));
        await act(async () => fireEvent.click(screen.getByText("Move to Done")));
        expect(screen.getByRole("list", { name: "In Progress" })).toHaveTextContent("Fix the login race");
    });
});
