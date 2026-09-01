import { describe, expect, it } from "vitest";
import { firstMatchingPage, searchSettings, settingsPage, SETTINGS_PAGES, SETTINGS_SECTIONS } from "./settingsCatalog";

describe("settings catalog", () => {
    it("keeps every section on a real page, with unique ids", () => {
        const ids = SETTINGS_SECTIONS.map((section) => section.id);
        expect(new Set(ids).size).toBe(ids.length);
        for (const section of SETTINGS_SECTIONS) expect(() => settingsPage(section.page)).not.toThrow();
        for (const page of SETTINGS_PAGES) expect(SETTINGS_SECTIONS.some((section) => section.page === page.id)).toBe(true);
    });

    it("matches every section on an empty query", () => {
        const { sections } = searchSettings("   ", true);
        expect(sections.size).toBe(SETTINGS_SECTIONS.length);
    });

    it("finds sections by title, body copy, and keyword synonyms", () => {
        expect(searchSettings("rail density", true).sections).toContain("agents.density");
        expect(searchSettings("unsandboxed", true).sections).toContain("commands.list");
        expect(searchSettings("hotkeys", true).sections).toContain("keybindings.map");
        expect(searchSettings("sso", true).sections).toContain("cloud.browser");
    });

    it("requires every term to match, ignoring case and spacing", () => {
        expect(searchSettings("  Update   CHANNEL ", true).sections).toEqual(new Set(["about.channel"]));
        expect(searchSettings("update nonsense", true).sections.size).toBe(0);
    });

    it("hides macOS-only sections off macOS", () => {
        expect(searchSettings("blur", true).sections).toContain("appearance.window");
        expect(searchSettings("blur", false).sections.size).toBe(0);
        expect(searchSettings("", false).counts.appearance).toBe(searchSettings("", true).counts.appearance! - 1);
    });

    it("counts matches per page and points at the first page holding one", () => {
        const matches = searchSettings("provider", true);
        expect(matches.counts.agents).toBe(2);
        expect(matches.counts.about).toBeUndefined();
        expect(firstMatchingPage(matches)).toBe("agents");
        expect(firstMatchingPage(searchSettings("nothing matches this", true))).toBeNull();
    });

    it("orders pages the way the rail renders them", () => {
        expect(SETTINGS_PAGES.map((page) => page.id)).toEqual([
            "general",
            "agents",
            "commands",
            "appearance",
            "keybindings",
            "cli",
            "cloud",
            "about",
        ]);
    });
});
