import { describe, expect, it } from "vitest";
import { documentLanguageOf } from "../languages";
import { languageFromPath, noteServedLanguages, uriToPath } from "./lsp";

describe("existing LSP API", () => {
    it("opens a file with any language server the native side can start", () => {
        noteServedLanguages(["typescript", "python", "lua"]);
        expect(languageFromPath("/project/file.tsx")).toBe("typescript");
        expect(documentLanguageOf("/project/file.tsx")).toBe("typescriptreact");
        expect(languageFromPath("/project/stubs.pyi")).toBe("python");
        expect(languageFromPath("/project/init.lua")).toBe("lua");
        expect(languageFromPath("/project/main.go")).toBeNull();
        expect(languageFromPath("/project/file.unknown")).toBeNull();
    });

    it("converts file URIs to paths", () => {
        expect(uriToPath("file:///project/a%20file.ts")).toBe("/project/a file.ts");
        expect(uriToPath("https://example.test/file.ts")).toBe("https://example.test/file.ts");
    });
});
