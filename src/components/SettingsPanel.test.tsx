import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { keybindingLabel, resolvedKeybinding } from "../keybindings";
import { getState, setState } from "../state/store";
import { SettingsPanel } from "./SettingsPanel";

const initial = getState();

beforeEach(() => {
    setState(initial, true);
    setState({ keybindingOverrides: {}, settingsOpen: true });
});

afterEach(cleanup);

describe("SettingsPanel keybindings", () => {
    it("records, blocks conflicts, clears, and resets shortcuts", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);
        await user.click(screen.getByRole("button", { name: "KeybindingsCommands and navigation" }));

        const projectDefault = keybindingLabel(resolvedKeybinding({}, "project.open"));
        const project = screen.getByRole("button", { name: `Open project: ${projectDefault}. Activate to change.` });
        await user.click(project);
        fireEvent.keyDown(project, { key: "Escape", code: "Escape" });
        expect(getState().settingsOpen).toBe(true);
        expect(screen.getByText("Change cancelled.")).toBeInTheDocument();

        await user.click(project);
        fireEvent.keyDown(project, { key: "o", code: "KeyO", ctrlKey: true, shiftKey: true });
        expect(getState().keybindingOverrides["project.open"]).toBe("Ctrl+Shift+KeyO");
        const replacementLabel = keybindingLabel("Ctrl+Shift+KeyO");
        expect(screen.getByRole("button", { name: `Open project: ${replacementLabel}. Activate to change.` })).toBeInTheDocument();

        const awsDefault = keybindingLabel(resolvedKeybinding({}, "aws.open"));
        const aws = screen.getByRole("button", { name: `Open AWS: ${awsDefault}. Activate to change.` });
        await user.click(aws);
        fireEvent.keyDown(aws, { key: "o", code: "KeyO", ctrlKey: true, shiftKey: true });
        expect(getState().keybindingOverrides["aws.open"]).toBeUndefined();
        expect(screen.getByText(`${replacementLabel} is already assigned to “Open project”.`)).toBeInTheDocument();

        fireEvent.keyDown(aws, { key: "Backspace", code: "Backspace" });
        expect(getState().keybindingOverrides["aws.open"]).toBeNull();
        expect(screen.getByRole("button", { name: "Open AWS: Unassigned. Activate to change." })).toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: "reset all" }));
        expect(getState().keybindingOverrides).toEqual({});
    });

    it("turning off tab restoration also clears and disables auto-resume", async () => {
        const user = userEvent.setup();
        setState({ restoreAgentTabs: true, autoResumeAgents: true });
        render(<SettingsPanel />);
        await user.click(screen.getByRole("button", { name: "AgentsRestore and status behavior" }));

        const restore = screen.getByRole("checkbox", { name: /Restore agent tabs/ });
        const autoResume = screen.getByRole("checkbox", { name: /Auto-resume restored agents/ });
        expect(autoResume).toBeChecked();

        await user.click(restore);
        expect(getState()).toMatchObject({ restoreAgentTabs: false, autoResumeAgents: false });
        expect(autoResume).not.toBeChecked();
        expect(autoResume).toBeDisabled();
    });

    it("mutes and unmutes notifications by agent type", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);
        await user.click(screen.getByRole("button", { name: "NotificationsAttention without noise" }));

        expect(screen.getByRole("checkbox", { name: /Agent notifications/ })).toBeChecked();
        expect(screen.getByRole("button", { name: "Enable native banners…" })).toBeInTheDocument();

        const claude = screen.getByRole("checkbox", { name: /Mute Claude/ });
        const codex = screen.getByRole("checkbox", { name: /Mute Codex/ });
        await user.click(claude);
        await user.click(codex);
        expect(getState().notificationPreferences.mutedAgentTypes).toEqual(["claude", "codex"]);

        await user.click(claude);
        expect(getState().notificationPreferences.mutedAgentTypes).toEqual(["codex"]);

        await user.selectOptions(screen.getByRole("combobox", { name: "signal tone" }), "bright");
        expect(getState().notificationPreferences.soundStyle).toBe("bright");
    });

    it("configures separate themes for system light and dark appearances", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);
        await user.click(screen.getByRole("button", { name: "AppearanceTheme and window" }));

        await user.selectOptions(screen.getByRole("combobox", { name: "Light appearance" }), "aura-day");
        await user.selectOptions(screen.getByRole("combobox", { name: "Dark appearance" }), "dracula");

        expect(getState()).toMatchObject({ systemLightThemeId: "aura-day", systemDarkThemeId: "dracula" });
    });
});
