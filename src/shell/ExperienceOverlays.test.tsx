import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: vi.fn(async () => "test") }));
vi.mock("../state/resources", () => ({
    useResourceEnabled: (enabled: boolean) => ({
        data: enabled
            ? [
                  { type: "codex", label: "Codex", available: false },
                  { type: "claude", label: "Claude" },
              ]
            : undefined,
        status: enabled ? ("ok" as const) : ("idle" as const),
        error: undefined,
        refresh: async () => {},
    }),
}));
vi.mock("../state/resources.defs", () => ({ agentCatalogR: { kind: "agents.catalog" } }));

import { keybindingLabel } from "../commands/keybindings";
import { flushPersist, resetPersistenceForTests } from "../state/persist";
import { getState, setState } from "../state/store";
import { uiActivity } from "../lib/activity";
import { DiagnosticsOverlay, Onboarding } from "./ExperienceOverlays";

const initial = getState();
const health = { git: true };

function openOnboarding(overrides = {}) {
    setState({ onboardingOpen: true, onboardingComplete: false, keybindingOverrides: overrides });
    return render(<Onboarding />);
}

async function expectPersistedComplete() {
    expect(await flushPersist()).toBe(true);
    const save = invoke.mock.calls.find(([command]) => command === "state_save");
    expect(save).toBeDefined();
    expect(JSON.parse(save![1].data as string).prefs.onboardingComplete).toBe(true);
}

beforeEach(() => {
    invoke.mockReset();
    invoke.mockImplementation(async (command: string) => (command === "integration_health" ? health : undefined));
    resetPersistenceForTests();
    setState(initial, true);
    setState({ onboardingOpen: false, onboardingComplete: false, keybindingOverrides: {} });
});

afterEach(() => {
    cleanup();
    resetPersistenceForTests();
});

describe("Onboarding", () => {
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
        expect(screen.getByRole("button", { name: "Close welcome" })).toHaveFocus();
    });

    it("runs a first move, closes, and persists completion", async () => {
        const user = userEvent.setup();
        openOnboarding();

        await user.click(screen.getByRole("button", { name: /Open a project/ }));

        expect(getState()).toMatchObject({ onboardingOpen: false, onboardingComplete: true, pickerOpen: true, pickerMode: "projects" });
        expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
        await expectPersistedComplete();
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
        expect(getState()).toMatchObject({ onboardingOpen: false, onboardingComplete: true });

        act(() => setState({ onboardingOpen: true, onboardingComplete: false }));
        await user.click(await screen.findByRole("button", { name: "Close welcome" }));
        expect(getState()).toMatchObject({ onboardingOpen: false, onboardingComplete: true });
        await expectPersistedComplete();
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

        act(() => setState({ onboardingOpen: true, onboardingComplete: true }));
        await waitFor(() => expect(screen.getByRole("button", { name: /Open a project/ })).toHaveFocus());
        await user.click(screen.getByRole("button", { name: "Close welcome" }));
        await waitFor(() => expect(trigger).toHaveFocus());
    });

    it("warns only when git is missing", async () => {
        openOnboarding();
        await waitFor(() => expect(invoke).toHaveBeenCalledWith("integration_health"));
        expect(screen.queryByText(/git not found/)).not.toBeInTheDocument();

        cleanup();
        invoke.mockImplementation(async (command: string) => (command === "integration_health" ? { git: false } : undefined));
        openOnboarding();
        expect(await screen.findByText("git not found: the Git view needs it")).toBeInTheDocument();
    });
});

describe("DiagnosticsOverlay", () => {
    it("shows what the interface was doing beside the raw snapshot", async () => {
        const now = vi.spyOn(performance, "now").mockReturnValue(0);
        uiActivity.reset();
        uiActivity.setSources({ focusPane: () => "editor", rejections: () => [{ message: "undefined is not an object", count: 7 }] });
        uiActivity.beginCommand("git_status");
        const settled = uiActivity.beginCommand("bruno_collection");
        now.mockReturnValue(2_500);
        uiActivity.endCommand(settled, false);
        setState({ diagnosticsOpen: true });

        const { container } = render(<DiagnosticsOverlay />);
        try {
            const stalled = await screen.findByText("git_status");
            expect(stalled.closest("span.is-stalled")).not.toBeNull();
            expect(screen.getByText("editor pane focused", { exact: false })).toBeInTheDocument();

            const failed = screen.getByText("bruno_collection").closest("span");
            expect(failed).toHaveClass("is-failed");
            expect(failed).toHaveTextContent("2500ms · failed");

            const rejection = screen.getByText("undefined is not an object").closest("span");
            expect(rejection).toHaveTextContent("×7");
            expect(container.querySelectorAll(".diagnostics-activity")).toHaveLength(1);
        } finally {
            now.mockRestore();
            uiActivity.setSources({ focusPane: () => null, rejections: () => [] });
            uiActivity.reset();
        }
    });
});
