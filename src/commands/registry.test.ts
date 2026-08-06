import { describe, expect, it, vi } from "vitest";
import { KEYBINDING_ACTIONS, keybindingLabel } from "../keybindings";
import { buildCommandRegistry, customCommandAvailable, type CustomCommand } from "./registry";

const custom: CustomCommand = {
    id: "tests",
    title: "Run tests",
    detail: "Run the focused test suite",
    command: "pnpm test",
    contexts: ["project"],
    placement: "split",
};

describe("command registry", () => {
    it("adapts every keybinding action and delegates execution", () => {
        const executeBuiltin = vi.fn();
        const entries = buildCommandRegistry({
            keybindingOverrides: { "project.open": "Ctrl+Shift+KeyO" },
            executeBuiltin,
        });

        expect(entries).toHaveLength(KEYBINDING_ACTIONS.length);
        const project = entries.find((entry) => entry.id === "project.open");
        expect(project?.shortcut).toBe(keybindingLabel("Ctrl+Shift+KeyO"));
        project?.execute();
        expect(executeBuiltin).toHaveBeenCalledWith("project.open");
    });

    it("filters contextual custom commands and delegates the original entry", () => {
        const executeCustom = vi.fn();
        const projectEntries = buildCommandRegistry({
            keybindingOverrides: {},
            executeBuiltin: vi.fn(),
            customCommands: [custom],
            executeCustom,
            context: "project",
        });
        const customEntry = projectEntries.find((entry) => entry.kind === "custom");
        expect(customEntry).toMatchObject({ title: "Run tests", placement: "split" });
        customEntry?.execute();
        expect(executeCustom).toHaveBeenCalledWith(custom);

        const commandEntries = buildCommandRegistry({
            keybindingOverrides: {},
            executeBuiltin: vi.fn(),
            customCommands: [custom],
            executeCustom,
            context: "command",
        });
        expect(commandEntries.some((entry) => entry.kind === "custom")).toBe(false);
    });

    it("treats an empty context list as globally available", () => {
        expect(customCommandAvailable({ ...custom, contexts: [] }, null)).toBe(true);
        expect(customCommandAvailable(custom, null)).toBe(false);
    });

    it("includes first-party actions that do not have keybindings", () => {
        const execute = vi.fn();
        const entries = buildCommandRegistry({
            keybindingOverrides: {},
            executeBuiltin: vi.fn(),
            standaloneCommands: [
                { id: "support.diagnostics", title: "Runtime diagnostics", detail: "Inspect runtime health", category: "Support", execute },
            ],
        });

        const diagnostics = entries.find((entry) => entry.id === "support.diagnostics");
        expect(diagnostics).toMatchObject({ kind: "standalone", shortcut: "", category: "Support" });
        expect(diagnostics?.searchText).toContain("Runtime diagnostics");
        diagnostics?.execute();
        expect(execute).toHaveBeenCalledOnce();
    });
});
