import { describe, expect, it } from "vitest";
import { safeWebUrl } from "./urls";

describe("safeWebUrl", () => {
    it("accepts only credential-free http(s) links", () => {
        expect(safeWebUrl("https://example.com/a?q=1")).toBe("https://example.com/a?q=1");
        expect(safeWebUrl("http://example.com")).toBe("http://example.com/");
        expect(safeWebUrl("file:///etc/passwd")).toBeNull();
        expect(safeWebUrl("javascript:alert(1)")).toBeNull();
        expect(safeWebUrl("https://user:secret@example.com")).toBeNull();
    });
});
