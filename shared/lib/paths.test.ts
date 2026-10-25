import { describe, expect, it } from "vitest";
import { basename, dirname, expandHome, isPathWithin, joinPath, prettyPath, relativePath } from "./paths";

describe("path helpers", () => {
    it("extracts basenames for unix-style paths", () => {
        expect(basename("/tmp/project/file.ts")).toBe("file.ts");
        expect(basename("/tmp/project/")).toBe("project");
        expect(basename("file.ts")).toBe("file.ts");
        expect(basename("")).toBe("");
        expect(basename("C:\\work\\sikemux\\src\\main.ts")).toBe("main.ts");
    });

    it("extracts dirnames without trailing slash noise", () => {
        expect(dirname("/tmp/project/file.ts")).toBe("/tmp/project");
        expect(dirname("/tmp/project/")).toBe("/tmp");
        expect(dirname("file.ts")).toBe("");
        expect(dirname("/")).toBe("");
        expect(dirname("C:\\work\\sikemux\\src\\main.ts")).toBe("C:\\work\\sikemux\\src");
    });

    it("joins and compares mixed Windows path separators", () => {
        expect(joinPath("C:\\work\\sikemux", "src", "main.ts")).toBe("C:/work/sikemux/src/main.ts");
        expect(relativePath("C:\\work\\sikemux\\src\\main.ts", "C:/work/sikemux")).toBe("src/main.ts");
        expect(isPathWithin("C:\\work\\sikemux-old", "C:\\work\\sikemux")).toBe(false);
    });

    it("pretty-prints and expands home paths", () => {
        expect(prettyPath("/Users/alice/proj", "/Users/alice")).toBe("~/proj");
        expect(prettyPath("/var/tmp", "/Users/alice")).toBe("/var/tmp");
        expect(prettyPath("/Users/alice2/proj", "/Users/alice")).toBe("/Users/alice2/proj");
        expect(prettyPath("/Users/alice", "/Users/alice")).toBe("~");
        expect(expandHome("~", "/Users/alice")).toBe("/Users/alice");
        expect(expandHome("~/proj", "/Users/alice")).toBe("/Users/alice/proj");
        expect(expandHome("/tmp", "/Users/alice")).toBe("/tmp");
    });
});
