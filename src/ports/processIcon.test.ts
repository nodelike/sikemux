import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { processIcon } from "./processIcon";

describe("processIcon", () => {
    it("knows a runtime by its process name, a path or a version suffix", () => {
        expect(processIcon("node")).toBe("nodejs");
        expect(processIcon("next-server (v14.2.3)")).toBe("nodejs");
        expect(processIcon("/usr/local/bin/python3.12")).toBe("python");
        expect(processIcon("uvicorn")).toBe("python");
    });

    it("has none for a process that names no runtime", () => {
        expect(processIcon("ai-service")).toBeNull();
        expect(processIcon("")).toBeNull();
    });

    it("names only icons the app ships", () => {
        for (const process of ["node", "deno", "python", "ruby", "java", "php", "elixir", "docker"]) {
            expect(existsSync(resolve(__dirname, `../../public/file-icons/${processIcon(process)}.svg`))).toBe(true);
        }
    });
});
