import { describe, expect, it } from "vitest";
import { hexDump } from "./HexView";

describe("hexDump", () => {
    it("lays out sixteen bytes a row with the printable text beside them", () => {
        const bytes = new TextEncoder().encode("Hello, world!\n\0\x01ABC");
        expect(hexDump(bytes).split("\n")).toEqual([
            "00000000  48 65 6c 6c 6f 2c 20 77  6f 72 6c 64 21 0a 00 01  Hello, world!...",
            "00000010  41 42 43                                          ABC",
        ]);
    });

    it("prints nothing for an empty file", () => {
        expect(hexDump(new Uint8Array())).toBe("");
    });
});
