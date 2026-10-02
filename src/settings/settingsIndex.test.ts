import { describe, expect, it } from "vitest";
import { searchSettings, SETTINGS_INDEX, SETTINGS_PAGE_NAMES, SETTINGS_PAGE_ORDER } from "./settingsIndex";

const labels = (query: string) => searchSettings(query, SETTINGS_INDEX).map((entry) => entry.label);

describe("searchSettings", () => {
    it("returns nothing for a blank query", () => {
        expect(searchSettings("   ", SETTINGS_INDEX)).toEqual([]);
    });

    it("matches words a setting does not show through its keywords", () => {
        expect(labels("font")).toEqual(["Text size"]);
        expect(labels("yolo")).toEqual(["Launch boundary"]);
    });

    it("needs every word, in any order", () => {
        expect(labels("codex profile")).toEqual(["Provider profiles", "Provider", "Codex default"]);
        expect(labels("codex nightly")).toEqual([]);
    });

    it("puts labels that start with the query first, then labels containing it", () => {
        expect(labels("profile").slice(0, 2)).toEqual(["Profile directory", "Provider profiles"]);
    });

    it("finds everything on a page by the page's name", () => {
        expect(labels("cloud")).toEqual(["Single sign-on", "Browser app", "Workspace shortcut"]);
    });

    it("names every page in the sidebar", () => {
        expect(Object.keys(SETTINGS_PAGE_NAMES).sort()).toEqual([...SETTINGS_PAGE_ORDER].sort());
    });
});
