import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: vi.fn(async () => "test") }));
vi.mock("../state/resources", () => ({
    useResourceEnabled: (enabled: boolean) => ({
        data: enabled
            ? [
                  { type: "claude", label: "Claude", status: { state: "signedOut" } },
                  { type: "codex", label: "Codex", available: false, status: { state: "missing" } },
                  { type: "hermes", label: "Hermes", available: false, status: { state: "missing" } },
                  { type: "grok", label: "Grok", available: false, status: { state: "broken", reason: "grok --version exited 1" } },
              ]
            : undefined,
        status: enabled ? ("ok" as const) : ("idle" as const),
        error: undefined,
        refresh: async () => {},
    }),
}));
vi.mock("../state/resources.defs", () => ({ agentSetupR: { kind: "agents.setup" } }));
const { installAgent, signInAgent } = vi.hoisted(() => ({ installAgent: vi.fn(), signInAgent: vi.fn() }));
vi.mock("../agents/AgentInstall", () => ({ installAgent, signInAgent }));

import { keybindingLabel } from "../commands/keybindings";
import { resetPersistenceForTests } from "../state/persist";
import { getState, setState } from "../state/store";
import { Onboarding } from "./ExperienceOverlays";
import { testPreferences } from "../test/preferences";

const initial = getState();
const health = { git: true };

function openOnboarding(overrides = {}) {
    setState({ onboardingOpen: true, preferences: testPreferences({ onboardingComplete: false, keybindingOverrides: overrides }) });
    return render(<Onboarding />);
}

function expectPersistedComplete() {
    expect(invoke).toHaveBeenCalledWith("core_call", { method: "setPreference", params: { key: "onboardingComplete", value: true } });
}

beforeEach(() => {
    invoke.mockReset();
    invoke.mockImplementation(async (command: string, args?: { method?: string }) =>
        command === "core_call" && args?.method === "hostTools" ? health : undefined,
    );
    resetPersistenceForTests();
    setState(initial, true);
    setState({ onboardingOpen: false, preferences: testPreferences({ onboardingComplete: false }) });
});

afterEach(() => {
    cleanup();
    resetPersistenceForTests();
});

describe("Onboarding", () => {
    it("offers Install for agents not on this Mac and Sign in for those signed out", async () => {
        openOnboarding();
        const hermes = (await screen.findByText("Hermes")).closest("li")!;
        expect(hermes).toHaveTextContent("Not installed");
        fireEvent.click(within(hermes).getByRole("button", { name: "Install" }));
        expect(installAgent).toHaveBeenCalledWith("hermes");

        const codex = screen.getByText("Codex").closest("li")!;
        fireEvent.click(within(codex).getByRole("button", { name: "Install" }));
        expect(installAgent).toHaveBeenLastCalledWith("codex");

        const claude = screen.getByText("Claude").closest("li")!;
        expect(within(claude).queryByRole("button", { name: "Install" })).not.toBeInTheDocument();
        fireEvent.click(within(claude).getByRole("button", { name: "Sign in" }));
        expect(signInAgent).toHaveBeenCalledWith(expect.objectContaining({ type: "claude" }));
        expect(within(hermes).queryByRole("button", { name: "Sign in" })).not.toBeInTheDocument();
        expect(screen.queryByText("Muse")).toBeNull();

        const grok = screen.getByText("Grok").closest("li")!;
        fireEvent.click(within(grok).getByRole("button", { name: "Reinstall" }));
        expect(installAgent).toHaveBeenLastCalledWith("grok");
    });

    it("focuses the first move and shows custom shortcuts", async () => {
        openOnboarding({ "project.open": "Ctrl+Shift+KeyO" });

        expect(screen.getByRole("dialog", { name: "Welcome to Sikemux" })).toBeInTheDocument();
        await waitFor(() => expect(screen.getByRole("button", { name: /Open a project/ })).toHaveFocus());
        expect(screen.getByText(keybindingLabel("Ctrl+Shift+KeyO"))).toBeInTheDocument();
    });

    it("moves between first moves with the arrow keys and keeps Tab inside", async () => {
        const user = userEvent.setup();
        openOnboarding();
        const project = screen.getByRole("button", { name: /Open a project/ });
        await waitFor(() => expect(project).toHaveFocus());

        fireEvent.keyDown(project, { key: "ArrowDown" });
        expect(screen.getByRole("button", { name: /Start an agent/ })).toHaveFocus();
        fireEvent.keyDown(document.activeElement!, { key: "ArrowUp" });
        fireEvent.keyDown(document.activeElement!, { key: "ArrowUp" });
        expect(screen.getByRole("button", { name: /Connect to a host/ })).toHaveFocus();

        await user.tab();
        expect(screen.getByRole("button", { name: "Sign in" })).toHaveFocus();
        await user.tab({ shift: true });
        expect(screen.getByRole("button", { name: /Connect to a host/ })).toHaveFocus();
    });

    it("runs a first move, closes, and persists completion", async () => {
        const user = userEvent.setup();
        openOnboarding();

        await user.click(screen.getByRole("button", { name: /Open a project/ }));

        expect(getState()).toMatchObject({ onboardingOpen: false, pickerOpen: true, pickerMode: "projects" });
        expect(getState().preferences?.onboardingComplete).toBe(true);
        expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
        expectPersistedComplete();
    });

    it("answers the shortcut shown beside a move", async () => {
        openOnboarding({ "project.open": "Ctrl+Shift+KeyO" });
        const project = screen.getByRole("button", { name: /Open a project/ });
        await waitFor(() => expect(project).toHaveFocus());

        fireEvent.keyDown(project, { code: "KeyO", ctrlKey: true, shiftKey: true });

        expect(getState()).toMatchObject({ onboardingOpen: false, pickerOpen: true, pickerMode: "projects" });
    });

    it("treats both Escape and the close button as persisted completion", async () => {
        const user = userEvent.setup();
        openOnboarding();

        fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
        expect(getState().onboardingOpen).toBe(false);
        expect(getState().preferences?.onboardingComplete).toBe(true);

        act(() => setState({ onboardingOpen: true, preferences: testPreferences({ onboardingComplete: false }) }));
        await user.click(await screen.findByRole("button", { name: "Close welcome" }));
        expect(getState().onboardingOpen).toBe(false);
        expect(getState().preferences?.onboardingComplete).toBe(true);
        expectPersistedComplete();
    });

    it("restores the previously focused control when it closes", async () => {
        const user = userEvent.setup();
        render(
            <>
                <button type="button">Reopen trigger</button>
                <Onboarding />
            </>,
        );
        const trigger = screen.getByRole("button", { name: "Reopen trigger" });
        trigger.focus();

        act(() => setState({ onboardingOpen: true, preferences: testPreferences() }));
        await waitFor(() => expect(screen.getByRole("button", { name: /Open a project/ })).toHaveFocus());
        await user.click(screen.getByRole("button", { name: "Close welcome" }));
        await waitFor(() => expect(trigger).toHaveFocus());
    });

    it("warns only when git is missing", async () => {
        openOnboarding();
        await waitFor(() => expect(invoke).toHaveBeenCalledWith("core_call", { method: "hostTools", params: null }));
        expect(screen.queryByText(/git not found/)).not.toBeInTheDocument();

        cleanup();
        invoke.mockImplementation(async (command: string, args?: { method?: string }) =>
            command === "core_call" && args?.method === "hostTools" ? { git: false } : undefined,
        );
        openOnboarding();
        expect(await screen.findByText("git not found: the Git view needs it")).toBeInTheDocument();
    });
});
