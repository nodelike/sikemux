import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { keybindingLabel, resolvedKeybinding } from "../keybindings";
import { IS_MACOS } from "../lib/platform";
import { getState, setState } from "../state/store";
import { SettingsPanel } from "./SettingsPanel";
import { searchSettings, settingsSection, SETTINGS_PAGES } from "./settingsCatalog";

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

    it("toggles agent tab restoration", async () => {
        const user = userEvent.setup();
        setState({ restoreAgentTabs: true });
        render(<SettingsPanel />);
        await user.click(screen.getByRole("button", { name: "AgentsProfiles and launch safety" }));

        const restore = screen.getByRole("switch", { name: /Restore agent tabs/ });
        expect(restore).toBeChecked();

        await user.click(restore);
        expect(getState()).toMatchObject({ restoreAgentTabs: false });
        expect(restore).not.toBeChecked();
    });

    it("persists an explicit launch boundary and non-secret provider path", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);
        await user.click(screen.getByRole("button", { name: "AgentsProfiles and launch safety" }));

        expect(screen.getAllByRole("radio").map((radio) => radio.textContent)).toEqual([
            expect.stringContaining("Normal"),
            expect.stringContaining("YOLO"),
        ]);
        await user.click(screen.getByRole("radio", { name: /YOLO/ }));
        expect(getState().defaultAgentPermissionMode).toBe("bypass");

        await user.click(screen.getByRole("button", { name: /Codexcodex.*system PATH/ }));
        await user.type(screen.getByRole("textbox", { name: "executable path" }), "/opt/codex/bin/codex");
        await user.click(screen.getByRole("button", { name: "save profile" }));

        expect(getState().providerProfiles.find((profile) => profile.id === "builtin-codex")?.executablePath).toBe("/opt/codex/bin/codex");
    });

    it("configures separate themes for system light and dark appearances", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);
        await user.click(screen.getByRole("button", { name: "AppearanceTheme and window" }));

        // The app dropdown is a button + listbox, not a native <select>.
        await user.click(screen.getByRole("button", { name: "Light appearance" }));
        await user.click(screen.getByRole("option", { name: /Aura Day/i }));
        await user.click(screen.getByRole("button", { name: "Dark appearance" }));
        await user.click(screen.getByRole("option", { name: /Dracula/i }));

        expect(getState()).toMatchObject({ systemLightThemeId: "aura-day", systemDarkThemeId: "dracula" });
    });
});

describe("SettingsPanel navigation", () => {
    const visible = searchSettings("", IS_MACOS);

    it("renders every catalogued section on its own page", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);

        for (const page of SETTINGS_PAGES) {
            await user.click(screen.getByRole("button", { name: `${page.name}${page.detail}` }));
            expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(page.name);
            for (const id of visible.sections) {
                const section = settingsSection(id);
                if (section.page !== page.id) continue;
                expect(screen.getByRole("heading", { level: 2, name: section.title })).toBeInTheDocument();
            }
        }
    });

    it("narrows the rail and the page to sections matching the search", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);

        await user.type(screen.getByRole("searchbox", { name: "Search settings" }), "hotkeys");

        expect(screen.getByRole("heading", { level: 2, name: "Command map" })).toBeInTheDocument();
        expect(screen.queryByRole("heading", { level: 2, name: "Project folders" })).not.toBeInTheDocument();
        expect(screen.getByRole("button", { name: /^KeybindingsCommands and navigation1$/ })).toBeInTheDocument();
        expect(screen.queryByRole("button", { name: /Projects and discovery/ })).not.toBeInTheDocument();
    });

    it("jumps from the search box into the first matching page", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);

        const box = screen.getByRole("searchbox", { name: "Search settings" });
        await user.type(box, "provider{Enter}");

        expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Agents");
        expect(screen.getByRole("button", { name: /^AgentsProfiles and launch safety2$/ })).toHaveFocus();
    });

    it("reports a search that matches nothing without losing the box", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);

        const box = screen.getByRole("searchbox", { name: "Search settings" });
        await user.type(box, "zzzz");

        expect(screen.getByText("Nothing matches that")).toBeInTheDocument();
        expect(box).toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: "Clear search" }));
        expect(box).toHaveValue("");
        for (const page of SETTINGS_PAGES) {
            expect(screen.getByRole("button", { name: `${page.name}${page.detail}` })).toBeInTheDocument();
        }
    });

    it("flips the action editor between new and editing", async () => {
        const user = userEvent.setup();
        setState({
            customCommands: [{ id: "command-demo", title: "Deploy", detail: "ship it", command: "make deploy", contexts: [], placement: "terminal" }],
        });
        render(<SettingsPanel />);
        await user.click(screen.getByRole("button", { name: "Command deckYour contextual actions" }));

        const head = () => screen.getByRole("heading", { level: 2, name: "Action editor" }).parentElement;
        expect(head()).toHaveTextContent("new");
        expect(screen.queryByRole("button", { name: /delete/ })).not.toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: /Deploy/ }));

        expect(head()).toHaveTextContent("editing");
        expect(screen.getByRole("button", { name: /delete/ })).toBeInTheDocument();
    });

    it("clears the search on Escape before closing settings", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);

        await user.type(screen.getByRole("searchbox", { name: "Search settings" }), "theme");
        fireEvent.keyDown(window, { key: "Escape" });

        expect(screen.getByRole("searchbox", { name: "Search settings" })).toHaveValue("");
        expect(getState().settingsOpen).toBe(true);

        fireEvent.keyDown(window, { key: "Escape" });
        expect(getState().settingsOpen).toBe(false);
    });

    it("walks pages with the arrow keys", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);

        await user.click(screen.getByRole("button", { name: "GeneralProjects and discovery" }));
        fireEvent.keyDown(screen.getByRole("navigation", { name: "Settings sections" }), { key: "ArrowDown" });
        expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Agents");

        fireEvent.keyDown(screen.getByRole("navigation", { name: "Settings sections" }), { key: "ArrowUp" });
        expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("General");
    });
});
