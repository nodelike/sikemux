import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { fileIconOf, fileIconTables, type FileIconIndex } from "./FileIcon";

const index = JSON.parse(readFileSync(resolve(__dirname, "../../public/file-icons/index.json"), "utf8")) as FileIconIndex;
const tables = fileIconTables(index);
const iconOf = (name: string) => fileIconOf(tables, name);

describe("fileIconOf", () => {
    it("matches a whole file name before its extension", () => {
        expect(iconOf("package.json")).toBe("nodejs");
        expect(iconOf(".gitignore")).toBe("git");
        expect(iconOf("data.json")).toBe("json");
    });

    it("prefers the longest extension", () => {
        expect(iconOf("index.d.ts")).toBe("typescript-def");
        expect(iconOf("index.ts")).toBe("typescript");
    });

    it("falls back to the language the app reads the file as", () => {
        expect(iconOf("App.tsx")).toBe("react_ts");
        expect(iconOf("main.rs")).toBe("rust");
        expect(iconOf("Dockerfile.dev")).toBe("docker");
    });

    it("gives an unknown file the plain file icon", () => {
        expect(iconOf("notes.unknownext")).toBe("file");
    });
});
