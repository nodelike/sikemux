import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
    available: vi.fn(),
    sessions: vi.fn(),
}));

vi.mock("../api/agents", () => ({
    agentApi: {
        available: mocks.available,
        sessions: mocks.sessions,
    },
}));

import { invalidate } from "../state/resources";
import { getState, setState } from "../state/store";
import { AgentPalette } from "./AgentPalette";

const initial = getState();

beforeEach(() => {
    setState(initial, true);
    const project = {
        id: "sess-project",
        name: "sikemux",
        kind: "project" as const,
        cwd: "/code/sikemux",
        deploy: null,
        pinned: false,
        activeWindowId: "win-project",
        activeAgentId: null,
        view: "agent" as const,
    };
    setState({
        sessions: { [project.id]: project },
        sessionOrder: [project.id],
        activeSessionId: project.id,
        agents: {},
        agentsBySession: { [project.id]: [] },
        agentPaletteOpen: true,
        defaultAgentPermissionMode: "workspace-write",
    });
    mocks.available.mockResolvedValue([
        { type: "codex", label: "Codex", command: "codex", defaultModel: null, defaultEffort: null },
        { type: "hermes", label: "Hermes", command: "hermes", defaultModel: null, defaultEffort: null },
        { type: "pi", label: "Pi", command: "pi", defaultModel: null, defaultEffort: null },
    ]);
    mocks.sessions.mockImplementation((type: string) => {
        if (type === "codex") return Promise.resolve([{ id: "codex-old", title: "Fix terminal tabs", mtime: 200 }]);
        if (type === "hermes") return Promise.resolve([{ id: "hermes-global", title: "Unrelated Hermes project", mtime: 300 }]);
        return Promise.resolve([{ id: "pi-old", title: "Review picker", mtime: 100 }]);
    });
    invalidate((kind) => kind === "agents.catalog");
});

afterEach(() => {
    cleanup();
    vi.clearAllMocks();
});

describe("AgentPalette", () => {
    it("restores the historical searchable picker and excludes Hermes project history", async () => {
        const opener = document.createElement("button");
        document.body.append(opener);
        opener.focus();
        const view = render(<AgentPalette />);

        const dialog = await screen.findByRole("dialog", { name: "Open agent CLI" });
        expect(dialog).toHaveClass("picker", "agent-palette");
        expect(screen.getByRole("textbox", { name: "Search agent sessions" })).toHaveFocus();
        expect(screen.getByRole("button", { name: "+ new Codex in Normal mode" })).toHaveClass("sel");
        expect(screen.getByRole("button", { name: "+ new Hermes in Normal mode" })).toBeInTheDocument();
        expect(await screen.findByRole("button", { name: "Fix terminal tabs in Normal mode" })).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Review picker in Normal mode" })).toBeInTheDocument();
        expect(screen.queryByText("Unrelated Hermes project")).not.toBeInTheDocument();
        expect(mocks.sessions).not.toHaveBeenCalledWith("hermes", expect.anything());
        expect(screen.queryByRole("textbox", { name: /task/i })).not.toBeInTheDocument();
        expect(screen.queryByText(/worktree/i)).not.toBeInTheDocument();
        expect(screen.getByRole("radio", { name: "safe" })).toBeChecked();
        expect(screen.getByRole("radio", { name: "yolo" })).not.toBeChecked();

        view.unmount();
        expect(opener).toHaveFocus();
        opener.remove();
    });

    it("opens armed when the saved default is YOLO", async () => {
        setState({ defaultAgentPermissionMode: "bypass" });
        render(<AgentPalette />);

        expect(await screen.findByRole("radio", { name: "yolo" })).toBeChecked();
        expect(screen.getByRole("button", { name: "+ new Codex in YOLO mode" })).toHaveClass("sel");
    });

    it("opens a new CLI directly in a PTY using Normal mode", async () => {
        const user = userEvent.setup();
        render(<AgentPalette />);

        await user.click(await screen.findByRole("button", { name: "+ new Codex in Normal mode" }));

        const id = getState().agentsBySession["sess-project"][0];
        expect(getState().agents[id]).toMatchObject({
            type: "codex",
            cwd: "/code/sikemux",
            permissionMode: "workspace-write",
            startup: "codex --sandbox workspace-write",
            directCommand: { program: "codex", args: ["--sandbox", "workspace-write"] },
        });
        expect(getState().agents[id]).not.toHaveProperty("initialInput");
        expect(getState().agents[id]).not.toHaveProperty("worktreePath");
        expect(getState().agentPaletteOpen).toBe(false);
    });

    it("uses the health-checked executable and selected profile directory", async () => {
        const user = userEvent.setup();
        setState({
            providerProfiles: [{ id: "codex-work", name: "Codex Work", provider: "codex", accent: "#10a37f", configPath: "~/.codex-work" }],
            selectedProviderProfileIds: { codex: "codex-work" },
        });
        mocks.available.mockResolvedValue([
            {
                type: "codex",
                label: "Codex",
                command: "/Applications/ChatGPT.app/Contents/Resources/codex",
                available: true,
                profileId: "codex-work",
                configPath: "~/.codex-work",
                defaultModel: null,
                defaultEffort: null,
            },
        ]);
        invalidate((kind) => kind === "agents.catalog");
        render(<AgentPalette />);

        await user.click(await screen.findByRole("button", { name: "+ new Codex in Normal mode" }));

        const id = getState().agentsBySession["sess-project"][0];
        expect(getState().agents[id]).toMatchObject({
            profileId: "codex-work",
            executablePath: "/Applications/ChatGPT.app/Contents/Resources/codex",
            directCommand: {
                program: "/Applications/ChatGPT.app/Contents/Resources/codex",
                profile: { configPath: "~/.codex-work" },
            },
        });
    });

    it("shows a broken selected profile without opening a dead terminal", async () => {
        const user = userEvent.setup();
        mocks.available.mockResolvedValue([
            {
                type: "codex",
                label: "Codex",
                command: "/opt/homebrew/bin/codex",
                available: false,
                error: "saved OpenCodex launcher is missing",
                defaultModel: null,
                defaultEffort: null,
            },
        ]);
        invalidate((kind) => kind === "agents.catalog");
        render(<AgentPalette />);

        expect(await screen.findByText("saved OpenCodex launcher is missing")).toBeInTheDocument();
        await user.click(screen.getByRole("button", { name: "+ new Codex in Normal mode" }));
        expect(getState().agentsBySession["sess-project"]).toEqual([]);
        expect(getState().agentPaletteOpen).toBe(true);
    });

    it("resumes a historical session with the selected mode", async () => {
        const user = userEvent.setup();
        render(<AgentPalette />);

        await user.click(screen.getByRole("radio", { name: "yolo" }));
        await user.click(await screen.findByRole("button", { name: "Fix terminal tabs in YOLO mode" }));

        const id = getState().agentsBySession["sess-project"][0];
        expect(getState().agents[id]).toMatchObject({
            type: "codex",
            title: "Fix terminal tabs",
            resumeId: "codex-old",
            permissionMode: "bypass",
            directCommand: {
                program: "codex",
                args: ["resume", "--dangerously-bypass-approvals-and-sandbox", "codex-old"],
            },
        });
    });

    it("toggles to YOLO and skips unsupported rows during keyboard navigation", async () => {
        const user = userEvent.setup();
        render(<AgentPalette />);

        const search = await screen.findByRole("textbox", { name: "Search agent sessions" });
        const yolo = screen.getByRole("radio", { name: "yolo" });
        yolo.focus();
        fireEvent.keyDown(yolo, { key: "Enter" });
        expect(getState().agentsBySession["sess-project"]).toEqual([]);
        await user.click(yolo);
        expect(yolo).toBeChecked();
        expect(screen.getByRole("button", { name: "+ new Pi in YOLO mode" })).toBeDisabled();
        fireEvent.keyDown(search, { key: "ArrowDown" });
        expect(screen.getByRole("button", { name: "+ new Hermes in YOLO mode" })).toHaveClass("sel");
        fireEvent.keyDown(search, { key: "ArrowDown" });
        expect(await screen.findByRole("button", { name: "Fix terminal tabs in YOLO mode" })).toHaveClass("sel");
    });

    it("filters sessions and opens the selected row with Enter", async () => {
        const user = userEvent.setup();
        render(<AgentPalette />);
        const search = await screen.findByRole("textbox", { name: "Search agent sessions" });
        await screen.findByRole("button", { name: "Review picker in Normal mode" });

        await user.type(search, "terminal tabs");
        expect(screen.queryByRole("button", { name: "+ new Codex in Normal mode" })).not.toBeInTheDocument();
        fireEvent.keyDown(search, { key: "Enter" });

        const id = getState().agentsBySession["sess-project"][0];
        expect(getState().agents[id]).toMatchObject({ resumeId: "codex-old", title: "Fix terminal tabs" });
    });

    it("retries CLI detection failures and only dismisses after leaving an empty agent view", async () => {
        const user = userEvent.setup();
        mocks.available
            .mockRejectedValueOnce(new Error("missing PATH"))
            .mockResolvedValueOnce([{ type: "codex", label: "Codex", command: "codex", defaultModel: null, defaultEffort: null }]);
        invalidate((kind) => kind === "agents.catalog");
        render(<AgentPalette />);

        expect(await screen.findByText(/missing PATH/)).toBeInTheDocument();
        await user.click(screen.getByRole("button", { name: "try again" }));
        await waitFor(() => expect(screen.getByRole("button", { name: "+ new Codex in Normal mode" })).toBeInTheDocument());
        fireEvent.keyDown(screen.getByRole("textbox", { name: "Search agent sessions" }), { key: "Escape" });
        expect(getState().agentPaletteOpen).toBe(true);

        setState((state) => ({
            sessions: { ...state.sessions, "sess-project": { ...state.sessions["sess-project"], view: "windows" } },
        }));
        fireEvent.keyDown(screen.getByRole("textbox", { name: "Search agent sessions" }), { key: "Escape" });
        expect(getState().agentPaletteOpen).toBe(false);
    });

    /*
     * The palette used to hear Escape only through its own onKeyDown, so
     * anything that held focus first — a terminal pane, which writes the escape
     * byte to its PTY and stops there — left it with no way out.
     */
    it("contains attempted outside focus and still captures Escape", async () => {
        setState((state) => ({
            sessions: { ...state.sessions, "sess-project": { ...state.sessions["sess-project"], view: "windows" } },
        }));
        render(<AgentPalette />);
        await waitFor(() => expect(screen.getByRole("textbox", { name: "Search agent sessions" })).toBeInTheDocument());

        const outsider = document.createElement("textarea");
        document.body.append(outsider);
        outsider.focus();
        expect(document.activeElement).not.toBe(outsider);
        expect(screen.getByRole("dialog")).toContainElement(document.activeElement as HTMLElement);

        fireEvent.keyDown(outsider, { key: "Escape" });

        expect(getState().agentPaletteOpen).toBe(false);
        outsider.remove();
    });
});
