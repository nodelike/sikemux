import { describe, expect, it } from "vitest";
import type { DiagnosticProblem } from "../workbench/diagnosticsController";
import { problemDelivery } from "./problemDelivery";

function problem(overrides: Partial<DiagnosticProblem>): DiagnosticProblem {
    return {
        project: "/repo",
        language: "typescript",
        path: "/repo/src/app.ts",
        serverGeneration: 1,
        version: 1,
        range: { start: { line: 3, character: 4 }, end: { line: 3, character: 8 } },
        severity: "error",
        code: "TS2304",
        source: "typescript",
        message: "Cannot find name 'x'.",
        ...overrides,
    } as DiagnosticProblem;
}

describe("problemDelivery", () => {
    it("names where the problem is and what reported it, and attaches the file", () => {
        expect(problemDelivery("/repo", problem({}))).toEqual({
            text: "Error at src/app.ts:4:5 from typescript TS2304:\n\nCannot find name 'x'.\n",
            paths: ["/repo/src/app.ts"],
        });
    });

    it("fences a message that runs over several lines", () => {
        const delivery = problemDelivery("/repo", problem({ severity: null, source: null, code: null, message: "Type 'a'\n  is not 'b'" }));
        expect(delivery.text).toBe("Problem at src/app.ts:4:5:\n\n```\nType 'a'\n  is not 'b'\n```\n");
    });
});
