import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const { installAgent, signInAgent } = vi.hoisted(() => ({ installAgent: vi.fn(), signInAgent: vi.fn() }));
vi.mock("../agents/AgentInstall", () => ({ installAgent, signInAgent }));
vi.mock("../state/resources", () => ({
    useResource: () => ({
        status: "ok",
        data: [
            { type: "opencode", label: "OpenCode", command: "opencode", available: true, status: { state: "ready", account: "apiKey" } },
            { type: "claude", label: "Claude", command: "claude", available: false, status: { state: "missing" } },
            { type: "codex", label: "Codex", command: "codex", available: true, status: { state: "signedOut" } },
            { type: "hermes", label: "Hermes", command: "hermes", available: false, status: { state: "broken", reason: "hermes --help exited 1" } },
            { type: "pi", label: "Pi", command: "pi", available: false, status: { state: "missing" } },
            { type: "omp", label: "OMP", command: "omp", available: false, status: { state: "missing" } },
            { type: "grok", label: "Grok", command: "grok", available: false, status: { state: "missing" } },
        ],
    }),
}));
vi.mock("../state/resources.defs", () => ({ agentSetupR: { kind: "agents.setup" } }));

import { AgentSetupSection } from "./AgentSetupSection";

afterEach(() => {
    cleanup();
    vi.clearAllMocks();
});

describe("Agents on this machine", () => {
    it("lists the agents as the core orders them, installs the missing and signs in the signed out", () => {
        render(<AgentSetupSection />);
        const rows = screen.getAllByRole("listitem");
        expect(rows.map((row) => row.querySelector("b")?.textContent)).toEqual(["OpenCode", "Claude", "Codex", "Hermes", "Pi", "OMP", "Grok"]);
        expect(screen.getByText("1 ready")).toBeInTheDocument();

        const claude = rows[1];
        expect(claude).toHaveTextContent("Not installed");
        expect(claude).toHaveTextContent("Anthropic's coding agent");
        fireEvent.click(within(claude).getByRole("button", { name: "Install" }));
        expect(installAgent).toHaveBeenCalledWith("claude");

        expect(within(rows[0]).queryByRole("button")).not.toBeInTheDocument();
        fireEvent.click(within(rows[2]).getByRole("button", { name: "Sign in" }));
        expect(signInAgent).toHaveBeenCalledWith(expect.objectContaining({ type: "codex" }));
    });

    it("offers to reinstall an agent that is broken, and says why", () => {
        render(<AgentSetupSection />);
        const hermes = screen.getByText("Hermes").closest("li")!;
        expect(hermes).toHaveTextContent("hermes --help exited 1");
        fireEvent.click(within(hermes).getByRole("button", { name: "Reinstall" }));
        expect(installAgent).toHaveBeenCalledWith("hermes");
    });
});
