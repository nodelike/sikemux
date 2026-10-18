import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { JiraIssue, JiraIssueSummary, JiraStatus } from "../api";

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
    assignee: { accountId: "a1", name: "Ana", avatarUrl: null },
    issueType: "Bug",
    sprint: "Sprint 5",
    updated: null,
    due: null,
    parent: null,
    url: "https://acme.atlassian.net/browse/ABC-12",
};

const detail: JiraIssue = {
    ...summary,
    project: "ABC",
    reporter: { accountId: "r1", name: "Raj", avatarUrl: null },
    labels: ["auth"],
    created: null,
    description: "Steps: **open** the app",
    comments: [{ id: "1", author: "Raj", authorAvatarUrl: null, created: "2026-10-04T09:00:00.000+0000", body: "Seen on staging" }],
    commentCount: 1,
    history: [],
    transitions: [{ id: "31", name: "Review", to: "In Review", toCategory: "indeterminate" }],
    subtasks: [],
    blockedBy: [],
    blocks: [],
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
    it("narrows the list to the statuses picked, and shows them all again from All", async () => {
        api.search.mockResolvedValue({
            issues: [
                summary,
                { ...summary, key: "ABC-13", summary: "Ship the docs", status: "Deployed", statusCategory: "done" },
                { ...summary, key: "ABC-14", summary: "Plan the beta", status: "To Do", statusCategory: "new" },
            ],
            next: null,
        });
        render(<JiraPane paneId="p" active />);
        const filter = await screen.findByRole("group", { name: "Filter by status" });
        expect(filter.textContent).toBe("All 3In Progress 1To Do 1Deployed 1");

        fireEvent.click(screen.getByRole("button", { name: "Deployed 1" }));
        expect(screen.getAllByRole("listitem").map((row) => row.textContent)).toEqual([expect.stringContaining("Ship the docs")]);

        fireEvent.click(screen.getByRole("button", { name: "To Do 1" }));
        expect(screen.getAllByRole("listitem")).toHaveLength(2);
        expect(screen.getByRole("button", { name: "All 3" })).toHaveAttribute("aria-pressed", "false");

        fireEvent.click(screen.getByRole("button", { name: "All 3" }));
        expect(screen.getAllByRole("listitem")).toHaveLength(3);
    });

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

    it("narrows a board to the people picked through Jira, and to typed words on the cards shown", async () => {
        const kiran = { ...summary, key: "ABC-20", summary: "Dossier chart", assignee: { accountId: "k1", name: "Kiran Rao", avatarUrl: null } };
        const loose = { ...summary, key: "ABC-21", summary: "Dossier picker", assignee: null };
        const columns = (issues: JiraIssueSummary[]) => [{ name: "In Progress", statusIds: ["3"], issues }];
        api.board.mockImplementation(async (_id: number, _site: string | undefined, picked: string[] = []) => ({
            id: 7,
            name: "ABC board",
            kind: "kanban",
            sprint: null,
            columns: columns(picked.includes("k1") ? [kiran] : [summary, kiran, loose]),
            truncated: false,
        }));
        render(<JiraPane paneId="jira-board-people" active />);
        fireEvent.click(await screen.findByRole("button", { name: "ABC board" }));
        expect(await screen.findAllByRole("listitem")).toHaveLength(3);

        fireEvent.click(screen.getByRole("button", { name: "Kiran Rao" }));
        await waitFor(() => expect(screen.getAllByRole("listitem")).toHaveLength(1));
        expect(api.board).toHaveBeenLastCalledWith(7, "acme.atlassian.net", ["k1"]);
        expect(screen.getByRole("button", { name: "Ana" })).toBeInTheDocument();

        fireEvent.click(screen.getByRole("button", { name: "Clear" }));
        await waitFor(() => expect(screen.getAllByRole("listitem")).toHaveLength(3));
        fireEvent.change(screen.getByRole("textbox", { name: "Find a card" }), { target: { value: "dossier" } });
        expect(screen.getAllByRole("listitem").map((card) => card.textContent)).toEqual([
            expect.stringContaining("Dossier chart"),
            expect.stringContaining("Dossier picker"),
        ]);
        expect(screen.getByRole("group", { name: "Filter the board" })).toHaveTextContent("Unassigned 1");
    });

    it("closes an opened issue back to the whole board", async () => {
        render(<JiraPane paneId="jira-board-close" active />);
        fireEvent.click(await screen.findByRole("button", { name: "ABC board" }));
        fireEvent.click(await screen.findByRole("listitem"));
        expect(await screen.findByRole("article", { name: "ABC-12 Fix the login race" })).toBeInTheDocument();

        fireEvent.click(screen.getByRole("button", { name: "Close the issue" }));
        expect(screen.queryByRole("article")).not.toBeInTheDocument();
        expect(screen.getByRole("list", { name: "In Progress" })).toHaveTextContent("Fix the login race");
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
