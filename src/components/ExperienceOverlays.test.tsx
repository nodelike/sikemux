import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: vi.fn(async () => "test") }));
vi.mock("../state/resources", () => ({
    useResourceEnabled: (enabled: boolean) => ({
        data: enabled ? [{ label: "Codex" }, { label: "Claude" }] : undefined,
        status: enabled ? ("ok" as const) : ("idle" as const),
        error: undefined,
        refresh: async () => {},
    }),
}));
vi.mock("../state/resources.defs", () => ({ agentCatalogR: { kind: "agents.catalog" } }));

import { keybindingLabel } from "../keybindings";
import * as cmd from "../state/commands";
import { flushPersist, resetPersistenceForTests } from "../state/persist";
import { getState, setState } from "../state/store";
import { uiActivity } from "../lib/activity";
import { DiagnosticsOverlay, Onboarding } from "./ExperienceOverlays";

const initial = getState();
const health = { shell: "/bin/zsh", git: true, aws: false, rnd: true };

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
    it("opens on the first scene, focuses the dialog, and renders canonical custom shortcuts", async () => {
        const user = userEvent.setup();
        openOnboarding({ "session.open": "Ctrl+Shift+KeyO" });

        const dialog = screen.getByRole("dialog", { name: "Everything you ship, one signal away" });
        expect(screen.getByLabelText("Onboarding step 1 of 4")).toBeInTheDocument();
        await waitFor(() => expect(dialog).toHaveFocus());

        await user.tab({ shift: true });
        expect(screen.getByRole("button", { name: "Continue" })).toHaveFocus();
        await user.tab();
        expect(screen.getByRole("button", { name: "Skip onboarding tour" })).toHaveFocus();

        await user.click(screen.getByRole("button", { name: "Continue" }));
        expect(screen.getByRole("heading", { name: "Open anything without breaking flow" })).toBeInTheDocument();
        expect(screen.getByText(keybindingLabel("Ctrl+Shift+KeyO"), { selector: "kbd" })).toBeInTheDocument();
    });

    it("supports click, arrow, progress, and Back navigation", async () => {
        const user = userEvent.setup();
        openOnboarding();
        const dialog = screen.getByRole("dialog");

        await user.click(screen.getByRole("button", { name: "Continue" }));
        expect(screen.getByLabelText("Onboarding step 2 of 4")).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Go to step 2: Muscle memory" })).toHaveAttribute("aria-current", "step");

        fireEvent.keyDown(dialog, { key: "ArrowRight" });
        expect(screen.getByRole("heading", { name: "Know when to watch, help, or move on" })).toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: "Go to step 4: Launch ready" }));
        expect(screen.getByLabelText("Onboarding step 4 of 4")).toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: "Back" }));
        expect(screen.getByLabelText("Onboarding step 3 of 4")).toBeInTheDocument();

        fireEvent.keyDown(dialog, { key: "ArrowLeft" });
        expect(screen.getByLabelText("Onboarding step 2 of 4")).toBeInTheDocument();
    });

    it("marks onboarding complete and persists it when the final scene finishes", async () => {
        const user = userEvent.setup();
        openOnboarding();

        await user.click(screen.getByRole("button", { name: "Go to step 4: Launch ready" }));
        await user.click(screen.getByRole("button", { name: "Enter Sikemux" }));

        expect(getState()).toMatchObject({ onboardingOpen: false, onboardingComplete: true });
        expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
        await expectPersistedComplete();
    });

    it("treats both Escape and Skip as persisted completion", async () => {
        const user = userEvent.setup();
        openOnboarding();

        fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
        expect(getState()).toMatchObject({ onboardingOpen: false, onboardingComplete: true });

        act(() => setState({ onboardingOpen: true, onboardingComplete: false }));
        await user.click(await screen.findByRole("button", { name: "Skip onboarding tour" }));
        expect(getState()).toMatchObject({ onboardingOpen: false, onboardingComplete: true });
        await expectPersistedComplete();
    });

    it("resets replay to step one and restores the previously focused control", async () => {
        const user = userEvent.setup();
        const { rerender } = render(
            <>
                <button type="button">Replay trigger</button>
                <Onboarding />
            </>,
        );
        const trigger = screen.getByRole("button", { name: "Replay trigger" });
        trigger.focus();

        act(() => setState({ onboardingOpen: true, onboardingComplete: true }));
        await waitFor(() => expect(screen.getByRole("dialog")).toHaveFocus());
        await user.click(screen.getByRole("button", { name: "Go to step 3: Agent signals" }));
        expect(screen.getByLabelText("Onboarding step 3 of 4")).toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: "Skip onboarding tour" }));
        await waitFor(() => expect(trigger).toHaveFocus());

        act(() => setState({ onboardingOpen: true }));
        rerender(
            <>
                <button type="button">Replay trigger</button>
                <Onboarding />
            </>,
        );
        expect(await screen.findByLabelText("Onboarding step 1 of 4")).toBeInTheDocument();
        await waitFor(() => expect(screen.getByRole("dialog")).toHaveFocus());
    });

    it("falls back cleanly when integration health rejects", async () => {
        const user = userEvent.setup();
        invoke.mockRejectedValueOnce(new Error("health unavailable"));
        openOnboarding();

        await user.click(screen.getByRole("button", { name: "Go to step 4: Launch ready" }));
        expect(await screen.findByText("Local tool check unavailable — setup can continue")).toBeInTheDocument();
        expect(invoke).toHaveBeenCalledWith("integration_health");
    });

    it("serializes global experience overlays and exposes textual health states", async () => {
        const user = userEvent.setup();
        setState({ onboardingOpen: false, diagnosticsOpen: true, whatsNewOpen: true });
        act(() => cmd.openOnboarding());
        expect(getState()).toMatchObject({ onboardingOpen: true, diagnosticsOpen: false, whatsNewOpen: false });

        render(<Onboarding />);
        await user.click(screen.getByRole("button", { name: "Go to step 4: Launch ready" }));
        expect(await screen.findByText("git ready")).toBeInTheDocument();
        expect(screen.getByText("aws missing")).toBeInTheDocument();

        act(() => cmd.openWhatsNew());
        expect(getState()).toMatchObject({ onboardingOpen: false, diagnosticsOpen: false, whatsNewOpen: true });
    });

    it("answers a real binding press by opening the matching overlay in the miniature", async () => {
        const user = userEvent.setup();
        const { container } = openOnboarding();

        await user.click(screen.getByRole("button", { name: "Go to step 2: Muscle memory" }));
        expect(screen.queryByText("open session")).not.toBeInTheDocument();
        expect(screen.getAllByText("press it")).toHaveLength(2);

        fireEvent.keyDown(window, { code: "KeyS", key: "s", altKey: true });
        expect(screen.getByText("open session")).toBeInTheDocument();
        expect(screen.getByText("✓ tried")).toBeInTheDocument();

        // Clicking a card is the mouse equivalent of pressing its binding.
        await user.click(screen.getByRole("button", { name: /Open command deck/ }));
        expect(screen.getByText("command deck")).toBeInTheDocument();
        expect(container.querySelectorAll(".onboarding-key.is-done")).toHaveLength(2);
    });

    it("points the miniature at the region the copy column describes", async () => {
        const user = userEvent.setup();
        const { container } = openOnboarding();
        const stage = container.querySelector(".onb-stage")!;
        expect(stage).toHaveAttribute("data-region", "none");

        await user.hover(screen.getByRole("button", { name: /Agent rail/ }));
        expect(stage).toHaveAttribute("data-region", "agents");

        await user.hover(screen.getByRole("button", { name: /Sessions rail/ }));
        expect(stage).toHaveAttribute("data-region", "rail");
    });

    it("runs the first move the final scene offers and closes the tour", async () => {
        const user = userEvent.setup();
        openOnboarding();

        await user.click(screen.getByRole("button", { name: "Go to step 4: Launch ready" }));
        await user.click(screen.getByRole("button", { name: /Open a project/ }));

        expect(getState()).toMatchObject({ onboardingOpen: false, onboardingComplete: true, pickerOpen: true, pickerMode: "projects" });
        await expectPersistedComplete();
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

    it("names the commands the overlay itself is waiting on", async () => {
        uiActivity.reset();
        setState({ diagnosticsOpen: true });
        render(<DiagnosticsOverlay />);

        expect(await screen.findByText("in flight")).toBeInTheDocument();
        expect(screen.getByText("slowest recent commands")).toBeInTheDocument();
        expect(screen.getByText("top rejection messages")).toBeInTheDocument();
        expect(screen.getByText("none")).toBeInTheDocument();
        expect(screen.getByText("runtime_diagnostics")).toBeInTheDocument();
    });
});
