import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useEffect } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PtyDirectCommand } from "../state/types/domain";

const mocks = vi.hoisted(() => ({
    exited: null as null | ((event: { payload: { id: number; code: number | null } }) => void),
    refreshStatuses: vi.fn(async () => {}),
    retryFailed: vi.fn(async () => {}),
    fetchResource: vi.fn(),
    invalidate: vi.fn(),
}));

vi.mock("../terminal/TerminalPane", () => ({
    TerminalPane: ({ directCommand, onPtySession }: { directCommand: PtyDirectCommand; onPtySession: (id: number) => void }) => {
        useEffect(() => onPtySession(7), [onPtySession]);
        return <pre data-testid="terminal">{directCommand.args.join(" ")}</pre>;
    },
}));
vi.mock("../api/transport", () => ({
    getIpcTransport: () => ({
        subscribe: async (_event: string, listener: typeof mocks.exited) => {
            mocks.exited = listener;
            return () => {};
        },
    }),
}));
vi.mock("../api/agents", () => ({ agentApi: { refreshStatuses: mocks.refreshStatuses, retryFailed: mocks.retryFailed } }));
vi.mock("../state/resources", () => ({ fetchResource: mocks.fetchResource, invalidate: mocks.invalidate }));
vi.mock("../state/resources.defs", () => ({ agentCatalogR: { kind: "agents.catalog" } }));

import { AgentInstallHost, installAgent, sheetCommand, signInAgent } from "./AgentInstall";

const exit = (id: number, code: number | null) => act(() => mocks.exited!({ payload: { id, code } }));

beforeEach(() => {
    mocks.fetchResource.mockResolvedValue([{ type: "hermes", label: "Hermes", available: true, status: { state: "ready", account: null } }]);
});

afterEach(() => {
    cleanup();
    vi.clearAllMocks();
});

describe("installing an agent", () => {
    it("runs the agent's own installer in a terminal and finds the agent once it succeeds", async () => {
        render(<AgentInstallHost />);
        act(() => installAgent("hermes"));

        expect(await screen.findByTestId("terminal")).toHaveTextContent("-c curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash");
        expect(screen.getByRole("dialog", { name: "Install Hermes" })).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();

        exit(8, 0);
        expect(mocks.refreshStatuses).not.toHaveBeenCalled();
        exit(7, 0);

        expect(await screen.findByRole("status")).toHaveTextContent("Hermes is installed: Ready.");
        expect(mocks.refreshStatuses).toHaveBeenCalledOnce();
        expect(mocks.invalidate).toHaveBeenCalled();
        fireEvent.click(screen.getByRole("button", { name: "Close" }));
        expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });

    it("says the installer failed and offers to run it again", async () => {
        render(<AgentInstallHost />);
        act(() => installAgent("grok"));
        await screen.findByTestId("terminal");

        exit(7, 1);
        expect(screen.getByRole("status")).toHaveTextContent("The installer stopped with exit code 1.");
        expect(mocks.refreshStatuses).not.toHaveBeenCalled();

        fireEvent.click(screen.getByRole("button", { name: "Try again" }));
        await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Running Grok's own installer…"));
        fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    });

    it("says so when the installer succeeded but the agent still cannot be found", async () => {
        mocks.fetchResource.mockResolvedValue([]);
        render(<AgentInstallHost />);
        act(() => installAgent("omp"));
        await screen.findByTestId("terminal");

        exit(7, 0);
        expect(await screen.findByText(/can't find OMP yet/)).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Close" }));
    });

    it("runs the agent's own sign-in on the account its profile names", () => {
        const signIn = (configPath: string | null) =>
            sheetCommand({ kind: "signIn", type: "claude", program: "/opt/bin/claude", configPath }, "/Users/me");
        expect(signIn(null)).toEqual({ direct: { program: "/opt/bin/claude", args: ["auth", "login"] }, shown: "claude auth login" });
        expect(signIn("~/.claude-work").direct).toEqual({
            program: "/usr/bin/env",
            args: ["CLAUDE_CONFIG_DIR=/Users/me/.claude-work", "/opt/bin/claude", "auth", "login"],
        });
        expect(signIn("/x/work/settings.json").direct.args[0]).toBe("CLAUDE_CONFIG_DIR=/x/work");
        expect(sheetCommand({ kind: "signIn", type: "opencode", program: "/bin/opencode", configPath: "/ignored" }, "/Users/me").direct).toEqual({
            program: "/bin/opencode",
            args: ["auth", "login"],
        });
    });

    it("says the agent is signed in once its sign-in succeeds", async () => {
        mocks.fetchResource.mockResolvedValue([
            { type: "claude", label: "Claude", available: true, status: { state: "ready", account: "subscription" } },
        ]);
        render(<AgentInstallHost />);
        act(() => signInAgent({ type: "claude", command: "/opt/bin/claude", configPath: null }));

        expect(await screen.findByTestId("terminal")).toHaveTextContent("auth login");
        expect(screen.getByRole("dialog", { name: "Sign in to Claude" })).toBeInTheDocument();
        exit(7, 0);
        expect(await screen.findByRole("status")).toHaveTextContent("Claude is signed in: Ready · subscription.");
        expect(mocks.refreshStatuses).toHaveBeenCalledOnce();
        fireEvent.click(screen.getByRole("button", { name: "Close" }));
    });

    it("runs a sign-in the agent offered, and sends the refused prompt again once it works", async () => {
        expect(
            sheetCommand(
                { kind: "signIn", type: "grok", program: "/g/grok", configPath: null, method: { args: ["login", "--web"], env: [["GROK_X", "1"]] } },
                "/Users/me",
            ),
        ).toEqual({ direct: { program: "/usr/bin/env", args: ["GROK_X=1", "/g/grok", "login", "--web"] }, shown: "grok login --web" });

        render(<AgentInstallHost />);
        act(() => signInAgent({ type: "grok", command: "/g/grok", configPath: null }, { method: { args: ["login"], env: [] }, retry: "grok-1" }));
        await screen.findByTestId("terminal");
        exit(7, 1);
        expect(mocks.retryFailed).not.toHaveBeenCalled();
        fireEvent.click(screen.getByRole("button", { name: "Try again" }));
        await screen.findByTestId("terminal");
        exit(7, 0);
        await waitFor(() => expect(mocks.retryFailed).toHaveBeenCalledWith("grok-1"));
        fireEvent.click(await screen.findByRole("button", { name: "Close" }));
    });

    it("says so when the agent still reads as signed out", async () => {
        mocks.fetchResource.mockResolvedValue([{ type: "codex", label: "Codex", available: true, status: { state: "signedOut" } }]);
        render(<AgentInstallHost />);
        act(() => signInAgent({ type: "codex", command: "/opt/bin/codex", configPath: null }));
        await screen.findByTestId("terminal");
        exit(7, 0);
        expect(await screen.findByRole("status")).toHaveTextContent("Codex still reads as signed out.");
        fireEvent.click(screen.getByRole("button", { name: "Close" }));
    });

    it("has nothing to run for an agent without an installer", () => {
        render(<AgentInstallHost />);
        act(() => installAgent("muse"));
        expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
});
