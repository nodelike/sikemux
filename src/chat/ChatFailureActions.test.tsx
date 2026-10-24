import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Agent } from "../state/types";

const { signInAgent, authenticateChat, retryFailed } = vi.hoisted(() => ({
    signInAgent: vi.fn(),
    authenticateChat: vi.fn(async () => {}),
    retryFailed: vi.fn(async () => {}),
}));
vi.mock("../agents/AgentInstall", () => ({ signInAgent }));
vi.mock("../api/agents", () => ({ agentApi: { authenticateChat, retryFailed } }));

import { ChatFailureActions } from "./ChatFailureActions";

const agent = (type: Agent["type"], executablePath?: string): Agent => ({ id: `${type}-1`, type, title: type, executablePath });

afterEach(() => {
    cleanup();
    vi.clearAllMocks();
});

describe("a chat that failed for want of a sign-in", () => {
    it("runs the agent's own sign-in for an agent without accounts", () => {
        render(<ChatFailureActions agent={agent("grok", "/Users/me/.grok/bin/grok")} profile={undefined} failure={{ kind: "signIn" }} />);
        fireEvent.click(screen.getByRole("button", { name: "Sign in to Grok" }));
        expect(signInAgent).toHaveBeenCalledWith({ type: "grok", command: "/Users/me/.grok/bin/grok", configPath: null }, { retry: "grok-1" });
    });

    it("offers the ways the agent said it can be signed in, by their own names", async () => {
        render(
            <ChatFailureActions
                agent={agent("grok", "/Users/me/.grok/bin/grok")}
                profile={undefined}
                failure={{ kind: "signIn" }}
                signInMethods={[
                    {
                        kind: "terminal",
                        id: "login",
                        name: "Log in with grok.com",
                        description: null,
                        args: ["login", "--web"],
                        env: [["GROK_X", "1"]],
                    },
                    { kind: "agent", id: "api-key", name: "Use an API key", description: "From XAI_API_KEY" },
                ]}
            />,
        );
        expect(screen.queryByRole("button", { name: "Sign in to Grok" })).not.toBeInTheDocument();

        fireEvent.click(screen.getByRole("button", { name: "Log in with grok.com" }));
        expect(signInAgent).toHaveBeenCalledWith(
            { type: "grok", command: "/Users/me/.grok/bin/grok", configPath: null },
            { method: { args: ["login", "--web"], env: [["GROK_X", "1"]] }, retry: "grok-1" },
        );

        fireEvent.click(screen.getByRole("button", { name: "Use an API key" }));
        expect(authenticateChat).toHaveBeenCalledWith("grok-1", "api-key");
        await waitFor(() => expect(retryFailed).toHaveBeenCalledWith("grok-1"));
    });

    it("offers nothing for other failures, or an agent with no sign-in to run", () => {
        const { container, rerender } = render(<ChatFailureActions agent={agent("opencode")} profile={undefined} failure={{ kind: "limit" }} />);
        expect(container).toBeEmptyDOMElement();
        rerender(<ChatFailureActions agent={agent("muse")} profile={undefined} failure={{ kind: "signIn" }} />);
        expect(container).toBeEmptyDOMElement();
    });
});
