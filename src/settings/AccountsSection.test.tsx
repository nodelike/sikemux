import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
    account: vi.fn(),
    addAccount: vi.fn(),
    signIn: vi.fn(),
    signOut: vi.fn(),
}));

vi.mock("../api/agents", () => ({
    agentApi: {
        account: mocks.account,
        addAccount: mocks.addAccount,
        signIn: mocks.signIn,
        signOut: mocks.signOut,
        onSignInPage: () => Promise.resolve(() => {}),
    },
}));

import { invalidate } from "../state/resources";
import { getState, setState } from "../state/store";
import { AccountsSections } from "./AccountsSection";

const initial = getState();

function signedIn(configPath?: string) {
    return {
        signedIn: true,
        name: null,
        email: configPath ? "work@example.com" : "me@example.com",
        plan: "max",
        organization: null,
        method: "subscription",
        sessions: "/home/me/.claude/projects",
    };
}

beforeEach(() => {
    setState(initial, true);
    mocks.account.mockImplementation(async (_agent: string, _executable?: string, configPath?: string) => signedIn(configPath));
    mocks.signIn.mockResolvedValue(undefined);
    mocks.signOut.mockResolvedValue(undefined);
    invalidate((kind) => kind === "agents.account" || kind === "agents.usage");
});

afterEach(() => {
    cleanup();
    vi.clearAllMocks();
});

describe("accounts in Settings", () => {
    it("says who each account is signed in as and which one new chats use", async () => {
        render(<AccountsSections />);

        expect(await screen.findAllByText("me@example.com · Max")).toHaveLength(2);
        expect(screen.getAllByText("New chats")).toHaveLength(2);
        expect(screen.queryByRole("switch")).not.toBeInTheDocument();
    });

    it("adds an account in a folder of its own and signs it in", async () => {
        mocks.addAccount.mockResolvedValue("~/.claude-work");
        const user = userEvent.setup();
        render(<AccountsSections />);

        const [claudeAdd] = await screen.findAllByRole("button", { name: "Add account" });
        await user.click(claudeAdd);
        await user.type(screen.getByRole("textbox", { name: "New Claude account name" }), "Work{Enter}");

        await waitFor(() => expect(mocks.addAccount).toHaveBeenCalledWith("claude", "Work"));
        await waitFor(() =>
            expect(getState().providerProfiles.some((profile) => profile.name === "Work" && profile.configPath === "~/.claude-work")).toBe(true),
        );
        await waitFor(() => expect(mocks.signIn).toHaveBeenCalledWith("claude", undefined, "~/.claude-work"));
    });

    it("picks the account new chats use, signs out, and turns on moving chats at a limit", async () => {
        setState((state) => ({
            providerProfiles: [
                ...state.providerProfiles,
                { id: "claude-work", name: "Work", provider: "claude", accent: "#e0a85a", configPath: "~/.claude-work" },
            ],
        }));
        const user = userEvent.setup();
        render(<AccountsSections />);

        await user.click(await screen.findByRole("button", { name: "Use for new chats" }));
        expect(getState().selectedProviderProfileIds.claude).toBe("claude-work");

        await user.click(screen.getByRole("switch", { name: /Move Claude chats to another account/ }));
        expect(getState().accountAutoSwitch.claude).toBe(true);

        const [signOut] = screen.getAllByRole("button", { name: "Sign out" });
        await user.click(signOut);
        await waitFor(() => expect(mocks.signOut).toHaveBeenCalled());
    });

    it("offers a sign-in for an account that is signed out", async () => {
        mocks.account.mockResolvedValue({ ...signedIn(), signedIn: false, email: null, plan: null });
        const user = userEvent.setup();
        render(<AccountsSections />);

        const [signIn] = await screen.findAllByRole("button", { name: "Sign in" });
        await user.click(signIn);
        await waitFor(() => expect(mocks.signIn).toHaveBeenCalledWith("claude", undefined, undefined));
    });
});
