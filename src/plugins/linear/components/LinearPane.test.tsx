import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { LinearIssue, LinearIssueDetail, LinearStatus } from "../api";

const api = vi.hoisted(() => ({
    status: vi.fn(),
    signIn: vi.fn(),
    signOut: vi.fn(),
    search: vi.fn(),
    issue: vi.fn(),
    comment: vi.fn(),
    update: vi.fn(),
}));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), linearApi: api }));

import { linear } from "../resources";
import { issueForAgent } from "../state";
import { LinearPane } from "./LinearPane";

const signedIn: LinearStatus = {
    configured: true,
    ok: true,
    authFailed: false,
    message: null,
    workspaces: [{ id: "o1", name: "Acme", urlKey: "acme", user: "ankit", isDefault: true }],
};

const todo = { id: "s1", name: "Todo", type: "unstarted", color: "#e2e2e2", position: 1 };
const review = { id: "s2", name: "In Review", type: "started", color: "#5e6ad2", position: 2 };

const listed: LinearIssue = {
    id: "i1",
    identifier: "ENG-12",
    title: "Fix the login flake",
    url: "https://linear.app/acme/issue/ENG-12",
    priority: 2,
    priorityLabel: "High",
    updatedAt: "2026-10-15T09:00:00Z",
    state: todo,
    assignee: null,
    team: { id: "t1", key: "ENG", name: "Engineering" },
    labels: [{ id: "l1", name: "bug", color: "#eb5757" }],
};

const detail: LinearIssueDetail = {
    ...listed,
    description: "It fails one run in ten.",
    createdAt: "2026-10-14T09:00:00Z",
    branchName: "ankit/eng-12-fix-the-login-flake",
    creator: { name: "Irwan" },
    team: { ...listed.team, states: [review, todo] },
    project: null,
    cycle: null,
    comments: [{ id: "c1", body: "Seen it on CI too.", createdAt: "2026-10-15T10:00:00Z", user: { name: "Irwan", displayName: "irwan" } }],
};

let pane = 0;
const renderPane = () => render(<LinearPane paneId={`linear-${++pane}`} active />);

afterEach(() => {
    cleanup();
    linear.refresh();
});

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    api.status.mockResolvedValue(signedIn);
    api.search.mockResolvedValue([listed]);
    api.issue.mockResolvedValue(detail);
    api.update.mockResolvedValue(detail);
    api.comment.mockResolvedValue({ id: "c2", url: "https://linear.app/c2" });
});

describe("the Linear pane", () => {
    it("offers the browser sign-in, and an API key instead, when no workspace is signed in", async () => {
        api.status.mockResolvedValue({ ...signedIn, configured: false, workspaces: [] });
        renderPane();
        expect(await screen.findByRole("heading", { name: "Connect Linear" })).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Continue with Linear" })).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Use an API key instead" }));
        expect(screen.getByPlaceholderText("lin_api_…")).toBeInTheDocument();
    });

    it("lists the person's open issues, and searches on Enter", async () => {
        renderPane();
        expect(await screen.findByText("Fix the login flake")).toBeInTheDocument();
        expect(screen.getByText("My open issues")).toBeInTheDocument();
        expect(api.search).toHaveBeenCalledWith("o1", "");

        fireEvent.change(screen.getByRole("textbox", { name: "Search Linear issues" }), { target: { value: "login" } });
        fireEvent.keyDown(screen.getByRole("textbox", { name: "Search Linear issues" }), { key: "Enter" });
        await waitFor(() => expect(api.search).toHaveBeenCalledWith("o1", "login"));
        expect(await screen.findByText("Matching “login”")).toBeInTheDocument();
    });

    it("opens an issue, moves it by state name, assigns it and comments", async () => {
        renderPane();
        fireEvent.click(await screen.findByRole("button", { name: /ENG-12/ }));
        expect(await screen.findByText("It fails one run in ten.")).toBeInTheDocument();
        expect(screen.getByText("Seen it on CI too.")).toBeInTheDocument();

        fireEvent.click(screen.getByRole("button", { name: "Assign to me" }));
        await waitFor(() => expect(api.update).toHaveBeenCalledWith("o1", "ENG-12", { assignee: "me" }));

        fireEvent.change(screen.getByRole("textbox", { name: "Comment on ENG-12" }), { target: { value: "On it" } });
        fireEvent.click(screen.getByRole("button", { name: "Comment" }));
        await waitFor(() => expect(api.comment).toHaveBeenCalledWith("o1", "ENG-12", "On it"));
    });

    it("hands an agent the issue with what it needs to update it", () => {
        const text = issueForAgent(detail);
        expect(text).toContain("Linear issue ENG-12: Fix the login flake — https://linear.app/acme/issue/ENG-12");
        expect(text).toContain("State: Todo");
        expect(text).toContain("Branch: ankit/eng-12-fix-the-login-flake");
        expect(text).toContain("**irwan**: Seen it on CI too.");
        expect(text).toContain("linear_update or linear_comment with id ENG-12");
    });
});
