import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { settingsApi } from "../api/settings";
import { keybindingLabel, resolvedKeybinding } from "../keybindings";
import { IS_MACOS } from "../lib/platform";
import { SETTINGS_INDEX, SETTINGS_PAGE_ORDER } from "../settingsIndex";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";
import { SettingsPanel } from "./SettingsPanel";

vi.mock("../themes/wallpaper", async (importOriginal) => ({
    ...(await importOriginal<typeof import("../themes/wallpaper")>()),
    wallpaperPixels: async () => {
        const pixels = new Uint8ClampedArray(32 * 32 * 4);
        for (let i = 0; i < pixels.length; i += 4) pixels.set(i % 64 < 8 ? [240, 70, 150, 255] : [10, 12, 24, 255], i);
        return pixels;
    },
}));

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
        await user.click(screen.getByRole("button", { name: "Keybindings" }));

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

        const sshDefault = keybindingLabel(resolvedKeybinding({}, "ssh.open"));
        const ssh = screen.getByRole("button", { name: `Connect to SSH host: ${sshDefault}. Activate to change.` });
        await user.click(ssh);
        fireEvent.keyDown(ssh, { key: "o", code: "KeyO", ctrlKey: true, shiftKey: true });
        expect(getState().keybindingOverrides["ssh.open"]).toBeUndefined();
        expect(screen.getByText(`${replacementLabel} is already assigned to “Open project”.`)).toBeInTheDocument();

        fireEvent.keyDown(ssh, { key: "Backspace", code: "Backspace" });
        expect(getState().keybindingOverrides["ssh.open"]).toBeNull();
        expect(screen.getByRole("button", { name: "Connect to SSH host: Unassigned. Activate to change." })).toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: "Reset all" }));
        expect(getState().keybindingOverrides).toEqual({});
    });

    it("toggles agent tab restoration", async () => {
        const user = userEvent.setup();
        setState({ restoreAgentTabs: true });
        render(<SettingsPanel />);
        await user.click(screen.getByRole("button", { name: "Agents" }));

        const restore = screen.getByRole("switch", { name: /Restore agent tabs/ });
        expect(restore).toBeChecked();

        await user.click(restore);
        expect(getState()).toMatchObject({ restoreAgentTabs: false });
        expect(restore).not.toBeChecked();
    });

    it("persists an explicit launch boundary and non-secret provider path", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);
        await user.click(screen.getByRole("button", { name: "Agents" }));

        expect(screen.getAllByRole("radio").map((radio) => radio.textContent)).toEqual([
            expect.stringContaining("Normal"),
            expect.stringContaining("YOLO"),
        ]);
        await user.click(screen.getByRole("radio", { name: /YOLO/ }));
        expect(getState().defaultAgentPermissionMode).toBe("bypass");

        await user.click(screen.getByRole("button", { name: /Codexcodex.*system PATH/ }));
        await user.type(screen.getByRole("textbox", { name: "executable path" }), "/opt/codex/bin/codex");
        await user.click(screen.getByRole("button", { name: "Save profile" }));

        expect(getState().providerProfiles.find((profile) => profile.id === "builtin-codex")?.executablePath).toBe("/opt/codex/bin/codex");
    });

    it("drafts a theme from the wallpaper and keeps it once saved", async () => {
        const user = userEvent.setup();
        vi.spyOn(settingsApi, "wallpaperImage").mockResolvedValue({ name: "Neon", dataUrl: "data:image/png;base64," });
        render(<SettingsPanel />);
        await user.click(screen.getByRole("button", { name: "Appearance" }));

        await user.click(screen.getByRole("button", { name: /From wallpaper/ }));
        expect(await screen.findByDisplayValue("Neon wallpaper")).toBeInTheDocument();
        expect(getState().customThemes).toHaveLength(0);

        await user.click(screen.getByRole("button", { name: "Save theme" }));
        expect(getState().customThemes.map((theme) => theme.name)).toEqual(["Neon wallpaper"]);
        expect(getState().themeId).toBe(getState().customThemes[0].id);
    });

    it("searches Ghostty's themes and applies them from the keyboard", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);
        await user.click(screen.getByRole("button", { name: "Appearance" }));

        const themes = screen.getByRole("listbox", { name: "Themes" });
        const search = screen.getByRole("textbox", { name: "Search themes" });
        await user.type(search, "rose pine");
        expect(
            within(themes)
                .getAllByRole("option")
                .map((option) => option.textContent),
        ).toEqual(["AaRose Pine", "AaRose Pine Dawn", "AaRose Pine Moon"]);

        await user.click(screen.getByRole("radio", { name: "light" }));
        expect(within(themes).getAllByRole("option")).toHaveLength(1);

        await user.type(search, "{ArrowDown}");
        expect(getState().themeId).toBe("ghostty-rose-pine-dawn");
        expect(within(themes).getByRole("option", { selected: true })).toHaveTextContent("Rose Pine Dawn");
    });
});

describe("SettingsPanel navigation", () => {
    it("indexes every section and row each page renders, and nothing it does not", () => {
        for (const page of SETTINGS_PAGE_ORDER) {
            setState({ settingsPage: page });
            const { container, unmount } = render(<SettingsPanel />);
            const rendered = [...container.querySelectorAll<HTMLElement>("[data-settings-target]")].map((element) => element.dataset.settingsTarget);
            const indexed = SETTINGS_INDEX.filter((entry) => entry.page === page).map((entry) => entry.target);
            expect(new Set(rendered), page).toEqual(new Set(indexed));
            unmount();
        }
    });

    it("finds a setting on another page and lands on it", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);
        const search = screen.getByRole("combobox", { name: "Search settings" });
        expect(search).toHaveFocus();

        await user.type(search, "sleep");
        expect(screen.getByRole("option", { name: /Idle agents/ })).toHaveAttribute("aria-selected", "true");

        await user.keyboard("{Enter}");
        expect(getState().settingsPage).toBe("agents");
        expect(search).toHaveValue("");
        const row = screen.getByRole("button", { name: "Sleep now" }).closest<HTMLElement>("[data-settings-target]");
        expect(row?.dataset.settingsTarget).toBe("Idle agents");
        expect(row).toHaveAttribute("data-settings-flash");
        expect(screen.getByRole("button", { name: "Sleep now" })).toHaveFocus();
    });

    it("moves through results with the arrow keys and opens a shortcut already filtered", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);
        await user.type(screen.getByRole("combobox", { name: "Search settings" }), "connect to ssh");
        const options = screen.getAllByRole("option");
        expect(options[0]).toHaveTextContent("Connect to SSH host");

        await user.keyboard("{Enter}");
        expect(getState().settingsPage).toBe("keybindings");
        expect(screen.getByRole("textbox", { name: "Filter shortcuts" })).toHaveValue("Connect to SSH host");

        await user.type(screen.getByRole("combobox", { name: "Search settings" }), "a");
        const first = screen.getAllByRole("option")[0];
        await user.keyboard("{ArrowDown}");
        expect(first).toHaveAttribute("aria-selected", "false");
        expect(screen.getAllByRole("option")[1]).toHaveAttribute("aria-selected", "true");
    });

    it("says so when nothing matches", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);
        await user.type(screen.getByRole("combobox", { name: "Search settings" }), "zzzz");
        expect(screen.getByText("No settings match “zzzz”.")).toBeInTheDocument();
    });

    it("clears the search on the first Escape and closes on the second", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);
        const search = screen.getByRole("combobox", { name: "Search settings" });
        await user.type(search, "blur");
        await user.keyboard("{Escape}");
        expect(search).toHaveValue("");
        expect(getState().settingsOpen).toBe(true);

        await user.keyboard("{Escape}");
        expect(getState().settingsOpen).toBe(false);
    });

    it("focuses the search from anywhere with the find shortcut", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);
        await user.click(screen.getByRole("button", { name: "Agents" }));
        expect(screen.getByRole("button", { name: "Agents" })).toHaveFocus();

        fireEvent.keyDown(window, { key: "f", code: "KeyF", metaKey: IS_MACOS, ctrlKey: !IS_MACOS });
        expect(screen.getByRole("combobox", { name: "Search settings" })).toHaveFocus();
    });

    it("walks the sidebar with the arrow keys, wrapping at the ends", async () => {
        const user = userEvent.setup();
        render(<SettingsPanel />);
        await user.click(screen.getByRole("button", { name: "General" }));

        await user.keyboard("{ArrowDown}");
        expect(getState().settingsPage).toBe("appearance");
        expect(screen.getByRole("button", { name: "Appearance" })).toHaveFocus();

        await user.keyboard("{ArrowUp}{ArrowUp}");
        expect(getState().settingsPage).toBe("plugins");

        await user.keyboard("{Home}");
        expect(getState().settingsPage).toBe("general");
    });

    it("reopens on the page it was left on, or the one asked for", async () => {
        const user = userEvent.setup();
        const { unmount } = render(<SettingsPanel />);
        await user.click(screen.getByRole("button", { name: "Cloud" }));
        unmount();

        render(<SettingsPanel />);
        expect(screen.getByRole("button", { name: "Cloud" })).toHaveAttribute("aria-current", "page");
        cleanup();

        cmd.openSettings("about");
        render(<SettingsPanel />);
        expect(screen.getByRole("button", { name: "About" })).toHaveAttribute("aria-current", "page");
        expect(screen.getByRole("button", { name: "Check now" })).toBeInTheDocument();
    });
});

describe("SettingsPanel beside the rest of the window", () => {
    it("leaves the sidebar next to it clickable", () => {
        render(
            <div>
                <aside>
                    <button type="button">Project</button>
                </aside>
                <main>
                    <SettingsPanel />
                </main>
            </div>,
        );
        expect(screen.getByRole("button", { name: "Project" }).closest("[inert]")).toBeNull();
    });
});
