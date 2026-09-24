import { describe, expect, it } from "vitest";
import { hasUnresolved, interpolate, mergeScope } from "./interpolate";

describe("Bruno interpolation", () => {
    it("merges scopes with earlier layers taking precedence", () => {
        expect(mergeScope({ token: "runtime", base: "https://runtime" }, { token: "env", other: "x" })).toEqual({
            token: "runtime",
            base: "https://runtime",
            other: "x",
        });
    });

    it("interpolates variables recursively", () => {
        const scope = { host: "example.test", baseUrl: "https://{{host}}", path: "v1" };
        expect(interpolate("{{baseUrl}}/{{ path }}", scope)).toBe("https://example.test/v1");
    });

    it("leaves unresolved normal variables intact but blanks process env variables", () => {
        expect(interpolate("{{missing}} {{process.env.NOPE}}", {})).toBe("{{missing}} ");
        expect(hasUnresolved("{{missing}}", {})).toBe(true);
        expect(hasUnresolved("{{known}}", { known: "ok" })).toBe(false);
    });
});
