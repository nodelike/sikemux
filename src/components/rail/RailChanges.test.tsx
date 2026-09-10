import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { RailChanges } from "./RailChanges";
import { useGitWorkbench } from "../../state/gitWorkbench";
import { git } from "../../api/git";

const resource = vi.hoisted(() => ({
    status: "ok",
    data: undefined as unknown,
    error: undefined as string | undefined,
    refresh: vi.fn(async () => {}),
}));
vi.mock("../../state/resources", async (original) => ({
    ...(await original<typeof import("../../state/resources")>()),
    useResourceEnabled: () => resource,
}));
vi.mock("../../api/git", async (original) => ({
    ...(await original<typeof import("../../api/git")>()),
    git: {
        push: vi.fn(async () => "Pushed to local remote"),
        pull: vi.fn(async () => "Already up to date"),
        fetch: vi.fn(async () => "Fetched"),
        stage: vi.fn(async () => {}),
        unstage: vi.fn(async () => {}),
        stageAll: vi.fn(async () => {}),
        status: vi.fn(),
        commit: vi.fn(),
    },
}));
vi.mock("../../state/git", () => ({ runGitCmd: (_label: string, action: () => Promise<unknown>) => action() }));

beforeEach(() => {
    vi.clearAllMocks();
    useGitWorkbench.setState({ drafts: {}, operations: {}, provider: "codex", model: "test" });
    resource.status = "ok";
    resource.error = undefined;
    resource.data = {
        status: { branch: "main", upstream: "origin/main", ahead: 2, behind: 1, files: [{ path: "src/file.ts", index: "M", worktree: "M" }] },
        branches: [],
        log: [],
    };
});
afterEach(cleanup);

describe("Changes rail workflows", () => {
    it("preserves drafts across navigation and isolates projects", async () => {
        const user = userEvent.setup();
        const view = render(<RailChanges cwd="/a" />);
        await user.type(screen.getByRole("textbox", { name: "Commit message" }), "A draft");
        view.rerender(<RailChanges cwd="/b" />);
        expect(screen.getByRole("textbox", { name: "Commit message" })).toHaveValue("");
        await user.type(screen.getByRole("textbox", { name: "Commit message" }), "B draft");
        view.unmount();
        render(<RailChanges cwd="/a" />);
        expect(screen.getByRole("textbox", { name: "Commit message" })).toHaveValue("A draft");
    });
    it("never reports an unread or failed repository as clean", () => {
        resource.data = undefined;
        resource.status = "loading";
        const view = render(<RailChanges cwd="/a" />);
        expect(screen.getByText("Reading repository…")).toBeInTheDocument();
        expect(screen.queryByText(/Working tree clean/)).not.toBeInTheDocument();
        resource.status = "error";
        resource.error = "Permission denied";
        view.rerender(<RailChanges cwd="/a" />);
        expect(screen.getByRole("alert")).toHaveTextContent("Permission denied");
        expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Push" })).toBeDisabled();
    });
    it("offers both stage and unstage for partially staged files", async () => {
        const user = userEvent.setup();
        render(<RailChanges cwd="/a" />);
        await user.click(screen.getByRole("button", { name: "Unstage file.ts" }));
        await waitFor(() => expect(git.unstage).toHaveBeenCalledWith("/a", "src/file.ts"));
        await user.click(screen.getByRole("button", { name: "Stage file.ts" }));
        expect(git.stage).toHaveBeenCalledWith("/a", "src/file.ts");
        expect(document.querySelector("button button")).toBeNull();
    });
    it("executes push, pull and fetch and retains their results", async () => {
        const user = userEvent.setup();
        render(<RailChanges cwd="/a" />);
        await user.click(screen.getByRole("button", { name: "Push 2" }));
        await screen.findByText("Pushed to local remote");
        expect(git.push).toHaveBeenCalledWith("/a");
        await user.click(screen.getByRole("button", { name: "Pull 1" }));
        await screen.findByText("Already up to date");
        await user.click(screen.getByRole("button", { name: "Fetch" }));
        await screen.findByText("Fetched");
        expect(git.fetch).toHaveBeenCalledWith("/a");
    });
});
