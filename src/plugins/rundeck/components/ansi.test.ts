import { describe, expect, it } from "vitest";
import { parseAnsi, stripAnsi } from "./ansi";

const ESC = "\x1b";

describe("parseAnsi", () => {
    it("returns plain text untouched", () => {
        expect(parseAnsi("hello")).toEqual([{ text: "hello", fg: null, bg: null, bold: false }]);
        expect(parseAnsi("")).toEqual([]);
    });

    it("maps standard, bright and background colours plus bold", () => {
        expect(parseAnsi(`${ESC}[31mred${ESC}[0m plain`)).toEqual([
            { text: "red", fg: 1, bg: null, bold: false },
            { text: " plain", fg: null, bg: null, bold: false },
        ]);
        expect(parseAnsi(`${ESC}[1;92mok${ESC}[22m!`)).toEqual([
            { text: "ok", fg: 10, bg: null, bold: true },
            { text: "!", fg: 10, bg: null, bold: false },
        ]);
        expect(parseAnsi(`${ESC}[44mbg${ESC}[49m`)).toEqual([{ text: "bg", fg: null, bg: 4, bold: false }]);
    });

    it("treats an empty SGR as a reset", () => {
        expect(parseAnsi(`${ESC}[33my${ESC}[mn`)).toEqual([
            { text: "y", fg: 3, bg: null, bold: false },
            { text: "n", fg: null, bg: null, bold: false },
        ]);
    });

    it("skips 256-colour and truecolour arguments without misreading them", () => {
        expect(parseAnsi(`${ESC}[38;5;31mx${ESC}[38;2;1;2;3;1my`)).toEqual([
            { text: "x", fg: null, bg: null, bold: false },
            { text: "y", fg: null, bg: null, bold: true },
        ]);
    });

    it("drops cursor, erase and title sequences", () => {
        expect(parseAnsi(`a${ESC}[2Kb${ESC}[1Ac${ESC}]0;title\x07d`)).toEqual([{ text: "abcd", fg: null, bg: null, bold: false }]);
        expect(stripAnsi(`${ESC}[32mdone${ESC}[0m`)).toBe("done");
    });
});
