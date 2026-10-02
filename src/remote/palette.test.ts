import { describe, expect, it } from "vitest";
import { plainColor } from "./palette";

describe("plainColor", () => {
    it("writes solid colours as hex and see-through ones as rgba", () => {
        expect(plainColor([15, 15, 19, 255])).toBe("#0f0f13");
        expect(plainColor([162, 119, 255, 36])).toBe("rgba(162, 119, 255, 0.14)");
    });
});
