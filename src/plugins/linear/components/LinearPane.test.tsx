import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { LinearStatus } from "../api";
import type { LinearIssueRow, LinearIssueView } from "../types.generated";

const api = vi.hoisted(() => ({
    status: vi.fn(),
    signIn: vi.fn(),
    signOut: vi.fn(),
    comment: vi.fn(),
    update: vi.fn(),
}));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), linearApi: api }));

const views = vi.hoisted(() => ({
    listView: vi.fn(),
    issueView: vi.fn(),
}));

import { fakeViews } from "../../../plugin-api/fakeViews";
import { linear } from "../resources";
import { LinearPane } from "./LinearPane";

const core = fakeViews(views);

const signedIn: LinearStatus = {
    configured: true,
    ok: true,
    authFailed: false,
    message: null,
    workspaces: [{ id: "o1", name: "Acme", urlKey: "acme", user: "ankit", isDefault: true }],
};

const todo = { id: "s1", name: "Todo", kind: "unstarted", color: "#e2e2e2" };
const review = { id: "s2", name: "In Review", kind: "started", color: "#5e6ad2" };

const listed: LinearIssueRow = { id: "i1", identifier: "ENG-12", title: "Fix the login flake", state: todo };

const detail: LinearIssueView = {
    id: "i1",
    identifier: "ENG-12",
    title: "Fix the login flake",
    url: "https://linear.app/acme/issue/ENG-12",
    branchName: "ankit/eng-12-fix-the-login-flake",
    state: todo,
    states: [todo, review],
    assignee: null,
    priority: "High",
    labels: [{ name: "bug", color: "#eb5757" }],
    description: "It fails one run in ten.",
    comments: [{ id: "c1", author: "irwan", createdAt: "2026-10-15T10:00:00Z", body: "Seen it on CI too." }],
    forAgent: "Linear issue ENG-12",
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
    Object.values(views).forEach((mock) => mock.mockReset());
    views.listView.mockImplementation(({ query }: { query: string }) => ({
        heading: query ? `Matching “${query}”` : "My open issues",
        empty: "No issue matches.",
        issues: [listed],
    }));
    views.issueView.mockReturnValue(detail);
    api.update.mockResolvedValue({ id: "i1" });
    api.comment.mockResolvedValue({ id: "c2" });
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
        expect(core.asked.at(-1)).toEqual({ method: "listView", params: { workspace: "o1", query: "" } });

        fireEvent.change(screen.getByRole("textbox", { name: "Search Linear issues" }), { target: { value: "login" } });
        fireEvent.keyDown(screen.getByRole("textbox", { name: "Search Linear issues" }), { key: "Enter" });
        await waitFor(() => expect(core.asked.at(-1)).toEqual({ method: "listView", params: { workspace: "o1", query: "login" } }));
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
});
