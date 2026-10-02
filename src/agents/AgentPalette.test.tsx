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

import { fetchResource, invalidate } from "../state/resources";
import { agentSessionsR } from "../state/resources.defs";
import { getState, setState } from "../state/store";
import { AgentPalette } from "./AgentPalette";
import { agentIdsOf } from "../state/selectors";

const initial = getState();

/** The picker only stays pinned while the project has nothing else to show. */
function openTerminal(): void {
    const pane = { type: "pane" as const, id: "win-project-pane", cwd: "/code/sikemux", kind: "terminal" as const, title: "1" };
    setState((state) => ({
        windows: { ...state.windows, "win-project": { id: "win-project", name: "1", role: "term" as const, root: pane, activePaneId: pane.id } },
        windowsBySession: { ...state.windowsBySession, "sess-project": ["win-project"] },
    }));
}

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
    };
    setState({
        sessions: { [project.id]: project },
        sessionOrder: [project.id],
        activeSessionId: project.id,
        agents: {},
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
        if (type === "hermes") return Promise.resolve([{ id: "hermes-old", title: "Generate a commit message", mtime: 300 }]);
        return Promise.resolve([{ id: "pi-old", title: "Review picker", mtime: 100 }]);
    });
    invalidate((kind) => kind === "agents.catalog" || kind === "agents.sessions");
});

afterEach(() => {
    cleanup();
    vi.clearAllMocks();
});

describe("AgentPalette", () => {
    it("restores the historical searchable picker with every agent's project history", async () => {
        const opener = document.createElement("button");
        document.body.append(opener);
        opener.focus();
        const view = render(<AgentPalette />);

        await screen.findByRole("dialog", { name: "Open agent CLI" });
        expect(screen.getByRole("textbox", { name: "Search agent sessions" })).toHaveFocus();
        expect(screen.getByRole("button", { name: "+ new Codex in Normal mode" })).toHaveClass("sel");
        expect(screen.getByRole("button", { name: "+ new Hermes in Normal mode" })).toBeInTheDocument();
        expect(await screen.findByRole("button", { name: "Fix terminal tabs in Normal mode" })).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Review picker in Normal mode" })).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Generate a commit message in Normal mode" })).toBeInTheDocument();
        expect(mocks.sessions).toHaveBeenCalledWith("hermes", "/code/sikemux", undefined);
        expect(screen.getByRole("radio", { name: "safe" })).toBeChecked();
        expect(screen.getByRole("radio", { name: "yolo" })).not.toBeChecked();

        view.unmount();
        expect(opener).toHaveFocus();
        opener.remove();
    });

    it("opens on the listings the running agents already read", async () => {
        await fetchResource(agentSessionsR, "codex", "/code/sikemux", undefined);
        let release = (_rows: { id: string; title: string; mtime: number }[]) => {};
        mocks.sessions.mockImplementation(
            (type: string) =>
                new Promise((resolve) => {
                    if (type === "codex") release = resolve;
                    else resolve([]);
                }),
        );

        render(<AgentPalette />);
        expect(await screen.findByRole("button", { name: "Fix terminal tabs in Normal mode" })).toBeInTheDocument();

        release([{ id: "codex-new", title: "Ship the picker", mtime: 400 }]);
        expect(await screen.findByRole("button", { name: "Ship the picker in Normal mode" })).toBeInTheDocument();
    });

    it("opens armed when the saved default is YOLO", async () => {
        setState({ defaultAgentPermissionMode: "bypass" });
        render(<AgentPalette />);

        expect(await screen.findByRole("radio", { name: "yolo" })).toBeChecked();
        expect(screen.getByRole("button", { name: "+ new Codex in YOLO mode" })).toHaveClass("sel");
    });

    it("follows a loaded default without replacing an explicit picker choice", async () => {
        const user = userEvent.setup();
        const view = render(<AgentPalette />);
        setState({ defaultAgentPermissionMode: "bypass" });
        view.rerender(<AgentPalette />);
        expect(await screen.findByRole("radio", { name: "yolo" })).toBeChecked();

        await user.click(screen.getByRole("radio", { name: "safe" }));
        view.rerender(<AgentPalette />);
        expect(screen.getByRole("radio", { name: "safe" })).toBeChecked();
    });

    it("opens a new CLI directly in a PTY using Normal mode", async () => {
        const user = userEvent.setup();
        render(<AgentPalette />);

        await user.click(await screen.findByRole("button", { name: "+ new Codex in Normal mode" }));

        const id = agentIdsOf(getState(), "sess-project")[0];
        expect(getState().agents[id]).toMatchObject({
            type: "codex",
            cwd: "/code/sikemux",
            permissionMode: "workspace-write",
            startup: "codex --sandbox workspace-write",
            directCommand: { program: "codex", args: ["--sandbox", "workspace-write"] },
        });
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
        invalidate((kind) => kind === "agents.catalog" || kind === "agents.sessions");
        render(<AgentPalette />);

        await user.click(await screen.findByRole("button", { name: "+ new Codex in Normal mode" }));

        const id = agentIdsOf(getState(), "sess-project")[0];
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
        invalidate((kind) => kind === "agents.catalog" || kind === "agents.sessions");
        render(<AgentPalette />);

        expect(await screen.findByText("saved OpenCodex launcher is missing")).toBeInTheDocument();
        await user.click(screen.getByRole("button", { name: "+ new Codex in Normal mode" }));
        expect(agentIdsOf(getState(), "sess-project")).toEqual([]);
        expect(getState().agentPaletteOpen).toBe(true);
    });

    it("resumes a historical session with the selected mode", async () => {
        const user = userEvent.setup();
        render(<AgentPalette />);

        await user.click(screen.getByRole("radio", { name: "yolo" }));
        await user.click(await screen.findByRole("button", { name: "Fix terminal tabs in YOLO mode" }));

        const id = agentIdsOf(getState(), "sess-project")[0];
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
        expect(agentIdsOf(getState(), "sess-project")).toEqual([]);
        await user.click(yolo);
        expect(yolo).toBeChecked();
        expect(screen.getByRole("button", { name: "+ new Pi in YOLO mode" })).toBeDisabled();
        fireEvent.keyDown(search, { key: "ArrowDown" });
        expect(screen.getByRole("button", { name: "+ new Hermes in YOLO mode" })).toHaveClass("sel");
        fireEvent.keyDown(search, { key: "ArrowDown" });
        expect(await screen.findByRole("button", { name: "Generate a commit message in YOLO mode" })).toHaveClass("sel");
    });

    it("filters sessions and opens the selected row with Enter", async () => {
        const user = userEvent.setup();
        render(<AgentPalette />);
        const search = await screen.findByRole("textbox", { name: "Search agent sessions" });
        await screen.findByRole("button", { name: "Review picker in Normal mode" });

        await user.type(search, "terminal tabs");
        expect(screen.queryByRole("button", { name: "+ new Codex in Normal mode" })).not.toBeInTheDocument();
        fireEvent.keyDown(search, { key: "Enter" });

        const id = agentIdsOf(getState(), "sess-project")[0];
        expect(getState().agents[id]).toMatchObject({ resumeId: "codex-old", title: "Fix terminal tabs" });
    });

    it("retries CLI detection failures and only dismisses after leaving an empty agent view", async () => {
        const user = userEvent.setup();
        mocks.available
            .mockRejectedValueOnce(new Error("missing PATH"))
            .mockResolvedValueOnce([{ type: "codex", label: "Codex", command: "codex", defaultModel: null, defaultEffort: null }]);
        invalidate((kind) => kind === "agents.catalog" || kind === "agents.sessions");
        render(<AgentPalette />);

        expect(await screen.findByText(/missing PATH/)).toBeInTheDocument();
        await user.click(screen.getByRole("button", { name: "try again" }));
        await waitFor(() => expect(screen.getByRole("button", { name: "+ new Codex in Normal mode" })).toBeInTheDocument());
        fireEvent.keyDown(screen.getByRole("textbox", { name: "Search agent sessions" }), { key: "Escape" });
        expect(getState().agentPaletteOpen).toBe(true);

        openTerminal();
        fireEvent.keyDown(screen.getByRole("textbox", { name: "Search agent sessions" }), { key: "Escape" });
        expect(getState().agentPaletteOpen).toBe(false);
    });

    /*
     * The palette used to hear Escape only through its own onKeyDown, so
     * anything that held focus first — a terminal pane, which writes the escape
     * byte to its PTY and stops there — left it with no way out.
     */
    it("contains attempted outside focus and still captures Escape", async () => {
        openTerminal();
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
