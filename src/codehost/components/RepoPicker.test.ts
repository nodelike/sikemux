import { describe, expect, it } from "vitest";
import type { RepoListing } from "../api";
import { entriesFor } from "./RepoPicker";

const NOW = Date.parse("2026-01-10T00:00:00Z");

const listing = (slug: string, extra: Partial<RepoListing> = {}): RepoListing => {
    const [owner = "", name = ""] = slug.split("/");
    return { owner, name, slug, private: false, archived: false, defaultBranch: "main", pushedAt: null, url: "", ...extra };
};

describe("entriesFor", () => {
    it("puts pinned repositories above the account's own", () => {
        const entries = entriesFor("github", ["me/pinned"], [listing("me/other")], NOW);
        expect(entries.map((entry) => entry.slug)).toEqual(["me/pinned", "me/other"]);
        expect(entries.map((entry) => entry.group)).toEqual(["Pinned", "Your repositories"]);
    });

    it("lists a pinned repository once, even when the account owns it too", () => {
        const entries = entriesFor("github", ["me/thing"], [listing("me/thing")], NOW);
        expect(entries).toHaveLength(1);
        expect(entries[0]?.group).toBe("Pinned");
    });

    it("says what is worth knowing about a repository under its name", () => {
        const [entry] = entriesFor("github", [], [listing("me/secret", { private: true, pushedAt: "2026-01-08T00:00:00Z" })], NOW);
        expect(entry?.sub).toBe("private · 2d ago");
    });

    it("leaves the line out rather than printing an empty one", () => {
        const [entry] = entriesFor("github", [], [listing("me/plain")], NOW);
        expect(entry?.sub).toBe("");
    });

    it("drops anything that is not one repository", () => {
        expect(entriesFor("github", ["not-a-slug", "a/b/c"], [], NOW)).toEqual([]);
    });
});
