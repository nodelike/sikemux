import { describe, expect, it } from "vitest";
import { MAX_ATTACHMENTS, entryName, mergePaths, projectEntries, rankEntries, removeToken, tokenAt } from "./composerInput";

describe("mergePaths", () => {
    it("adds new paths after the ones already attached", () => {
        expect(mergePaths(["/a"], ["/b", "/c"])).toEqual(["/a", "/b", "/c"]);
    });

    it("skips duplicates, empty paths and paths with a null byte", () => {
        expect(mergePaths(["/a"], ["/a", "", "/b\0c", "/b", "/b"])).toEqual(["/a", "/b"]);
    });

    it("stops at the attachment limit", () => {
        const full = Array.from({ length: MAX_ATTACHMENTS - 1 }, (_, index) => `/f${index}`);
        const merged = mergePaths(full, ["/x", "/y"]);
        expect(merged).toHaveLength(MAX_ATTACHMENTS);
        expect(merged.at(-1)).toBe("/x");
    });

    it("leaves the current list untouched", () => {
        const current = ["/a"];
        mergePaths(current, ["/b"]);
        expect(current).toEqual(["/a"]);
    });
});

describe("tokenAt with a slash", () => {
    it("finds a command at the start of the draft", () => {
        expect(tokenAt("/rev", 4)).toEqual({ trigger: "/", start: 0, needle: "rev" });
        expect(tokenAt("/", 1)).toEqual({ trigger: "/", start: 0, needle: "" });
    });

    it("finds a command part-way through a sentence", () => {
        expect(tokenAt("please /comp", 12)).toEqual({ trigger: "/", start: 7, needle: "comp" });
    });

    it("reads only up to the caret", () => {
        expect(tokenAt("/review this", 4)).toEqual({ trigger: "/", start: 0, needle: "rev" });
    });

    it("ignores a slash inside a word or path", () => {
        expect(tokenAt("src/main", 8)).toBeNull();
    });

    it("stops once the caret has moved past the command", () => {
        expect(tokenAt("/review this", 12)).toBeNull();
    });

    it("finds nothing at the start of the draft or without a slash", () => {
        expect(tokenAt("/review", 0)).toBeNull();
        expect(tokenAt("hello", 5)).toBeNull();
    });
});

describe("tokenAt", () => {
    it("names the trigger the caret sits after", () => {
        expect(tokenAt("look at @src/ma", 15)).toEqual({ trigger: "@", start: 8, needle: "src/ma" });
        expect(tokenAt("fix #12", 7)).toEqual({ trigger: "#", start: 4, needle: "12" });
        expect(tokenAt("#", 1)).toEqual({ trigger: "#", start: 0, needle: "" });
        expect(tokenAt("/rev", 4)).toEqual({ trigger: "/", start: 0, needle: "rev" });
    });

    it("leaves a trigger inside a word as text", () => {
        expect(tokenAt("me@x.com", 8)).toBeNull();
        expect(tokenAt("C#", 2)).toBeNull();
        expect(tokenAt("a/b", 3)).toBeNull();
        expect(tokenAt("see src/@types", 14)).toBeNull();
    });

    it("finds several in one message, each where the caret is", () => {
        const text = "@a.ts and @b.ts";
        expect(tokenAt(text, 5)).toEqual({ trigger: "@", start: 0, needle: "a.ts" });
        expect(tokenAt(text, text.length)).toEqual({ trigger: "@", start: 10, needle: "b.ts" });
        expect(tokenAt(text, 9)).toBeNull();
    });

    it("keeps the other characters inside a token's needle", () => {
        expect(tokenAt("@src/main", 9)).toEqual({ trigger: "@", start: 0, needle: "src/main" });
    });
});

describe("removeToken", () => {
    it("takes the token out and leaves the caret where it was", () => {
        expect(removeToken("look at @src/ma", { start: 8 }, 15)).toEqual({ text: "look at ", caret: 8 });
    });

    it("drops the space after a token so no double space is left", () => {
        expect(removeToken("@a.ts and more", { start: 0 }, 5)).toEqual({ text: "and more", caret: 0 });
        expect(removeToken("fix #1 now", { start: 4 }, 6)).toEqual({ text: "fix now", caret: 4 });
    });
});

describe("projectEntries", () => {
    it("lists the files, then each folder once", () => {
        expect(projectEntries(["src/a.ts", "src/lib/b.ts", "README.md"])).toEqual([
            { path: "src/a.ts", folder: false },
            { path: "src/lib/b.ts", folder: false },
            { path: "README.md", folder: false },
            { path: "src/", folder: true },
            { path: "src/lib/", folder: true },
        ]);
    });
});

describe("rankEntries", () => {
    const entries = projectEntries(["src/chat/ChatComposer.tsx", "src/chat/reducer.ts", "docs/chat.md"]);

    it("matches by name and by path", () => {
        expect(rankEntries("composer", entries, 10).map((entry) => entry.path)).toEqual(["src/chat/ChatComposer.tsx"]);
        expect(rankEntries("src/chat", entries, 10)).toContainEqual({ path: "src/chat/", folder: true });
    });

    it("offers folders by name", () => {
        expect(entryName({ path: "src/chat/", folder: true })).toBe("chat");
        expect(rankEntries("docs", entries, 10)[0]).toEqual({ path: "docs/", folder: true });
    });

    it("stops at the limit", () => {
        expect(rankEntries("", entries, 2)).toHaveLength(2);
    });
});
