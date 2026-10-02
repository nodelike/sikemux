import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetResourcesForTests } from "../state/resources";
import { getState, setState } from "../state/store";
import { ProjectStrip } from "./ProjectStrip";

const mocks = vi.hoisted(() => ({
    move: vi.fn(),
    scan: vi.fn(async () => [
        { name: "app", path: "/work/app" },
        { name: "notes", path: "/work/notes" },
    ]),
}));

vi.mock("../state/commands", () => ({ moveAgentToProject: mocks.move }));
vi.mock("../api/git", () => ({ git: { overview: async () => ({ status: { branch: "main" } }) } }));
vi.mock("../api/settings", () => ({ settingsApi: { scanProjectRoots: mocks.scan } }));

const initial = getState();
const toggle = vi.fn();

function renderStrip(state: Parameters<typeof ProjectStrip>[0]["worktree"]["state"] = { kind: "choosing", on: false }) {
    return render(<ProjectStrip agentId="agent-1" cwd="/work/app" worktree={{ state, toggle }} />);
}

beforeEach(() => {
    vi.clearAllMocks();
    resetResourcesForTests();
    setState(
        {
            ...initial,
            home: "/home/me",
            projectRoots: [{ path: "/work", depth: 1 }],
            sessions: {
                a: { id: "a", name: "app", kind: "project", cwd: "/work/app", pinned: false, activeWindowId: "" },
                s: { id: "s", name: "site", kind: "project", cwd: "/work/site", pinned: false, activeWindowId: "" },
            },
            sessionOrder: ["a", "s"],
        },
        true,
    );
});

afterEach(cleanup);

describe("ProjectStrip", () => {
    it("names the project, its branch and the worktree switch", async () => {
        renderStrip();
        expect(screen.getByRole("button", { name: "Project" })).toHaveTextContent("app");
        expect(await screen.findByText("main")).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "worktree" }));
        expect(toggle).toHaveBeenCalledTimes(1);
    });

    it("drops the worktree switch outside a git repository", () => {
        renderStrip({ kind: "hidden" });
        expect(screen.queryByRole("button", { name: "worktree" })).not.toBeInTheDocument();
    });

    it("lists open projects before ones found on disk, and moves the chat to the one picked", async () => {
        renderStrip();
        fireEvent.click(screen.getByRole("button", { name: "Project" }));
        const list = screen.getByRole("listbox", { name: "Projects" });
        await waitFor(() => expect(within(list).getAllByRole("option")).toHaveLength(3));
        expect(
            within(list)
                .getAllByRole("option")
                .map((option) => option.querySelector("strong")?.textContent),
        ).toEqual(["app", "site", "notes"]);
        expect(within(list).getByRole("option", { selected: true })).toHaveTextContent("app");

        fireEvent.click(within(list).getByText("notes"));
        expect(mocks.move).toHaveBeenCalledWith("agent-1", "/work/notes");
        expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    it("filters as you type and picks the first match on Enter", async () => {
        renderStrip();
        fireEvent.click(screen.getByRole("button", { name: "Project" }));
        const search = screen.getByRole("combobox", { name: "Search projects" });
        await waitFor(() => expect(screen.getAllByRole("option")).toHaveLength(3));
        fireEvent.change(search, { target: { value: "site" } });
        expect(screen.getAllByRole("option")).toHaveLength(1);
        fireEvent.keyDown(search, { key: "Enter" });
        expect(mocks.move).toHaveBeenCalledWith("agent-1", "/work/site");
    });

    it("does nothing when the chat's own project is picked", () => {
        renderStrip();
        fireEvent.click(screen.getByRole("button", { name: "Project" }));
        fireEvent.click(within(screen.getByRole("listbox")).getByText("app"));
        expect(mocks.move).not.toHaveBeenCalled();
    });
});
