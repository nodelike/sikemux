import { describe, expect, it } from "vitest";
import type { Pull } from "./api";
import { needsPull } from "./compose";

const pull = (head: string, state = "open") => ({ head, state }) as Pull;

describe("needsPull", () => {
    it("offers a branch that has no open pull request", () => {
        expect(needsPull("feat/x", [pull("feat/y")], ["main"])).toBe(true);
        expect(needsPull("feat/x", [pull("feat/x", "closed")], ["main"])).toBe(true);
    });

    it("stays quiet for a branch already in review, or one that is itself a base", () => {
        expect(needsPull("feat/x", [pull("feat/x")], ["main"])).toBe(false);
        expect(needsPull("main", [], [])).toBe(false);
        expect(needsPull("release/1", [], ["release/1"])).toBe(false);
        expect(needsPull(null, [], [])).toBe(false);
    });
});
