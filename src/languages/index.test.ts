import { describe, expect, it } from "vitest";
import { grammarFor, languageOf } from ".";

describe("grammarFor", () => {
    it("reads a language by name, alias or file", () => {
        expect(grammarFor("rust")).toBe("rust");
        expect(grammarFor("sh")).toBe("shellscript");
        expect(grammarFor("src/pages/index.astro")).toBe("astro");
        expect(grammarFor("App.vue")).toBe("vue");
        expect(grammarFor("Main.kt")).toBe("kotlin");
        expect(grammarFor("src/lib.rs:12:4")).toBe("rust");
        expect(grammarFor("C:\\repo\\include\\util.H")).toBe("c");
        expect(grammarFor("script.pl")).toBe("perl");
        expect(grammarFor("paper.tex")).toBe("latex");
    });

    it("knows files by their whole name", () => {
        expect(grammarFor("Dockerfile")).toBe("docker");
        expect(grammarFor("Dockerfile.dev")).toBe("docker");
        expect(grammarFor("Makefile")).toBe("make");
        expect(grammarFor(".env.local")).toBe("dotenv");
        expect(grammarFor("CMakeLists.txt")).toBe("cmake");
    });

    it("reads JavaScript with the TypeScript grammar the app ships, though the file is still JavaScript", () => {
        expect(languageOf("index.js")).toBe("javascript");
        expect(grammarFor("index.js")).toBe("typescript");
        expect(grammarFor("App.tsx")).toBe("typescript");
        expect(grammarFor("jsx")).toBe("typescript");
    });

    it("has nothing for a file no grammar reads", () => {
        expect(grammarFor("")).toBeNull();
        expect(grammarFor("notes.txt")).toBeNull();
        expect(grammarFor("LICENSE")).toBeNull();
        expect(grammarFor("archive.unknownext")).toBeNull();
    });
});
