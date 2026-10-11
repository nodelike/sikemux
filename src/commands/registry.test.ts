import { describe, expect, it } from "vitest";
import { customCommandAvailable, type CustomCommand } from "./registry";

const custom: CustomCommand = {
    id: "tests",
    title: "Run tests",
    detail: "Run the focused test suite",
    command: "pnpm test",
    contexts: ["project"],
    placement: "split",
};

describe("command registry", () => {
    it("treats an empty context list as globally available", () => {
        expect(customCommandAvailable({ ...custom, contexts: [] }, null)).toBe(true);
        expect(customCommandAvailable(custom, null)).toBe(false);
    });
});
