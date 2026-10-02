import { describe, expect, it, vi } from "vitest";
import { LSP_PAYLOAD_LIMITS, type LspDiagnostic, type LspDiagnosticsPayload, type LspRange } from "../api/lsp";
import { DIAGNOSTICS_CONTROLLER_LIMITS, DiagnosticsController } from "./diagnosticsController";

function diagnostic(
    message: string,
    severity: LspDiagnostic["severity"] = "warning",
    line = 1,
    character = 0,
    rangeOverride?: LspRange,
): LspDiagnostic {
    return {
        range: rangeOverride ?? {
            start: { line, character },
            end: { line, character: character + 1 },
        },
        severity,
        code: null,
        source: "test",
        message,
    };
}

function payload(
    path: string,
    version: number | null,
    diagnostics: readonly LspDiagnostic[] = [diagnostic("problem")],
    overrides: Partial<LspDiagnosticsPayload> = {},
): LspDiagnosticsPayload {
    return {
        project: "/repo",
        language: "typescript",
        path,
        version,
        diagnostics,
        ...overrides,
    };
}

describe("DiagnosticsController publish policy", () => {
    it("requires the owned project and an explicitly activated generation", () => {
        const controller = new DiagnosticsController("/repo");

        expect(controller.publish(payload("/repo/a.ts", 1), 1)).toBe("generation-mismatch");
        expect(controller.activateServer("typescript", 1)).toBe(true);
        expect(controller.activateServer("typescript", 1)).toBe(false);
        expect(() => controller.activateServer("typescript", 0)).toThrow(RangeError);
        expect(controller.publish(payload("/repo/a.ts", 1, undefined, { project: "/other" }), 1)).toBe("wrong-project");
        expect(controller.publish(payload("/repo/a.ts", 1), 0)).toBe("invalid-generation");
        expect(controller.publish(payload("/repo/a.ts", 1), 2)).toBe("generation-mismatch");
        expect(controller.publish(payload("/repo/a.ts", 1), 1)).toBe("applied");

        expect(controller.selectDocument("/repo/a.ts")).toMatchObject([
            { project: "/repo", path: "/repo/a.ts", language: "typescript", serverGeneration: 1, version: 1 },
        ]);
    });

    it("rejects older numbered versions and retains clear tombstones", () => {
        const controller = new DiagnosticsController("/repo");
        controller.activateServer("typescript", 1);

        expect(controller.publish(payload("/repo/a.ts", 5, [diagnostic("five")]), 1)).toBe("applied");
        expect(controller.publish(payload("/repo/a.ts", 4, [diagnostic("four")]), 1)).toBe("stale-version");
        expect(controller.selectDocument("/repo/a.ts").map(({ message }) => message)).toEqual(["five"]);

        expect(controller.publish(payload("/repo/a.ts", 5, [diagnostic("equal replacement")]), 1)).toBe("applied");
        expect(controller.publish(payload("/repo/a.ts", 6, []), 1)).toBe("applied");
        expect(controller.selectDocument("/repo/a.ts")).toEqual([]);
        expect(controller.getSnapshot()).toMatchObject({ documents: 0, problems: 0 });

        expect(controller.publish(payload("/repo/a.ts", 5, [diagnostic("stale resurrection")]), 1)).toBe("stale-version");
        expect(controller.publish(payload("/repo/a.ts", 7, [diagnostic("fresh")]), 1)).toBe("applied");
        expect(controller.selectDocument("/repo/a.ts").map(({ message }) => message)).toEqual(["fresh"]);
    });

    it("accepts null versions in arrival order until a numbered version is known", () => {
        const controller = new DiagnosticsController("/repo");
        controller.activateServer("typescript", 1);

        expect(controller.publish(payload("/repo/a.ts", null, [diagnostic("unversioned one")]), 1)).toBe("applied");
        expect(controller.publish(payload("/repo/a.ts", null, [diagnostic("unversioned two")]), 1)).toBe("applied");
        expect(controller.selectDocument("/repo/a.ts").map(({ message }) => message)).toEqual(["unversioned two"]);

        expect(controller.publish(payload("/repo/a.ts", 1, [diagnostic("numbered")]), 1)).toBe("applied");
        expect(controller.publish(payload("/repo/a.ts", null, [diagnostic("ambiguous stale")]), 1)).toBe("unversioned-after-numbered");
        expect(controller.selectDocument("/repo/a.ts").map(({ message }) => message)).toEqual(["numbered"]);

        expect(controller.activateServer("typescript", 2)).toBe(true);
        expect(controller.publish(payload("/repo/a.ts", null, [diagnostic("new generation")]), 2)).toBe("applied");
        expect(controller.selectDocument("/repo/a.ts")).toMatchObject([{ message: "new generation", serverGeneration: 2, version: null }]);
    });

    it("never lets an old or shut-down server generation affect its replacement", () => {
        const controller = new DiagnosticsController("/repo");
        controller.activateServer("typescript", 10);
        controller.publish(payload("/repo/a.ts", 1, [diagnostic("generation ten")]), 10);

        expect(controller.activateServer("typescript", 11)).toBe(true);
        expect(controller.selectProblems()).toEqual([]);
        expect(controller.publish(payload("/repo/a.ts", 99, [diagnostic("late ten")]), 10)).toBe("generation-mismatch");
        expect(controller.publish(payload("/repo/a.ts", 1, [diagnostic("generation eleven")]), 11)).toBe("applied");
        expect(controller.shutdownServer("typescript", 10)).toBe(false);
        expect(controller.selectProblems()).toHaveLength(1);

        expect(controller.shutdownServer("typescript", 11)).toBe(true);
        expect(controller.getSnapshot()).toMatchObject({ activeServers: 0, documents: 0, problems: 0 });
        expect(controller.publish(payload("/repo/a.ts", 2), 11)).toBe("generation-mismatch");
        expect(controller.activateServer("typescript", 11)).toBe(false);
        expect(controller.activateServer("typescript", 12)).toBe(true);
    });

    it("isolates independent language servers for the same project and path", () => {
        const controller = new DiagnosticsController("/repo");
        controller.activateServer("typescript", 1);
        controller.activateServer("eslint", 7);
        controller.publish(payload("/repo/a.ts", 1, [diagnostic("typescript")]), 1);
        controller.publish(payload("/repo/a.ts", 2, [diagnostic("eslint")], { language: "eslint" }), 7);

        expect(controller.selectDocument("/repo/a.ts").map(({ language }) => language)).toEqual(["eslint", "typescript"]);
        expect(controller.shutdownServer("typescript", 1)).toBe(true);
        expect(controller.selectDocument("/repo/a.ts")).toMatchObject([{ language: "eslint", message: "eslint" }]);
    });

    it("rejects malformed typed bypasses and bounds aggregate storage", () => {
        const controller = new DiagnosticsController("/repo");
        controller.activateServer("typescript", 1);
        const oversized = Array.from({ length: LSP_PAYLOAD_LIMITS.maxDiagnostics + 1 }, () => diagnostic("oversized"));
        expect(controller.publish(payload("/repo/oversized.ts", 1, oversized), 1)).toBe("invalid-payload");

        const fullDocument = Array.from({ length: LSP_PAYLOAD_LIMITS.maxDiagnostics }, (_value, index) => diagnostic(`problem ${index}`));
        const documentsAtCapacity = DIAGNOSTICS_CONTROLLER_LIMITS.maxStoredDiagnostics / LSP_PAYLOAD_LIMITS.maxDiagnostics;
        for (let index = 0; index < documentsAtCapacity; index += 1) {
            expect(controller.publish(payload(`/repo/${index}.ts`, 1, fullDocument), 1)).toBe("applied");
        }
        expect(controller.getSnapshot().problems).toBe(DIAGNOSTICS_CONTROLLER_LIMITS.maxStoredDiagnostics);
        expect(controller.selectProblems()).toHaveLength(DIAGNOSTICS_CONTROLLER_LIMITS.maxStoredDiagnostics);
        expect(controller.publish(payload("/repo/overflow.ts", 1), 1)).toBe("capacity");
    });
});

describe("DiagnosticsController clearing and selectors", () => {
    it("clears one document across servers without losing version watermarks", () => {
        const controller = new DiagnosticsController("/repo");
        controller.activateServer("typescript", 1);
        controller.activateServer("eslint", 2);
        controller.publish(payload("/repo/a.ts", 5, [diagnostic("ts a")]), 1);
        controller.publish(payload("/repo/a.ts", 6, [diagnostic("eslint a")], { language: "eslint" }), 2);
        controller.publish(payload("/repo/b.ts", 3, [diagnostic("ts b")]), 1);

        expect(controller.clearDocument("/repo/a.ts")).toBe(2);
        expect(controller.clearDocument("/repo/a.ts")).toBe(0);
        expect(controller.selectDocument("/repo/a.ts")).toEqual([]);
        expect(controller.selectProblems().map(({ message }) => message)).toEqual(["ts b"]);
        expect(controller.publish(payload("/repo/a.ts", 4, [diagnostic("old ts a")]), 1)).toBe("stale-version");
        expect(controller.publish(payload("/repo/a.ts", 7, [diagnostic("new ts a")]), 1)).toBe("applied");

        expect(controller.clearProject()).toBe(2);
        expect(controller.clearProject()).toBe(0);
        expect(controller.selectProblems()).toEqual([]);
        expect(controller.getSnapshot()).toMatchObject({ documents: 0, problems: 0, activeServers: 2 });
    });

    it("returns cached immutable selectors with deterministic Problems and editor orders", () => {
        const controller = new DiagnosticsController("/repo");
        controller.activateServer("typescript", 1);
        controller.activateServer("eslint", 2);
        const mutableRange = {
            start: { line: 5, character: 2 },
            end: { line: 5, character: 3 },
        };
        controller.publish(payload("/repo/b.ts", 1, [diagnostic("b error", "error", 5, 2, mutableRange)]), 1);
        controller.publish(
            payload("/repo/a.ts", 2, [
                diagnostic("late warning", "warning", 10),
                diagnostic("early error", "error", 2),
                diagnostic("first unknown", null, 1),
            ]),
            1,
        );
        controller.publish(payload("/repo/a.ts", 3, [diagnostic("eslint hint", "hint", 4)], { language: "eslint" }), 2);
        mutableRange.start.line = 99;

        const problems = controller.selectProblems();
        const editor = controller.selectDocument("/repo/a.ts");
        const documents = controller.selectDocuments();

        expect(problems.map(({ message }) => message)).toEqual(["early error", "b error", "late warning", "eslint hint", "first unknown"]);
        expect(editor.map(({ message }) => message)).toEqual(["first unknown", "early error", "eslint hint", "late warning"]);
        expect(documents.map(({ path, language }) => `${path}:${language}`)).toEqual([
            "/repo/a.ts:eslint",
            "/repo/a.ts:typescript",
            "/repo/b.ts:typescript",
        ]);
        expect(documents[1].diagnostics.map(({ message }) => message)).toEqual(["first unknown", "early error", "late warning"]);
        expect(problems.find(({ message }) => message === "b error")?.range.start.line).toBe(5);

        expect(controller.selectProblems()).toBe(problems);
        expect(controller.selectDocument("/repo/a.ts")).toBe(editor);
        expect(controller.selectDocuments()).toBe(documents);
        expect(Object.isFrozen(problems)).toBe(true);
        expect(Object.isFrozen(problems[0])).toBe(true);
        expect(Object.isFrozen(editor)).toBe(true);
        expect(Object.isFrozen(documents)).toBe(true);
        expect(Object.isFrozen(documents[0].diagnostics)).toBe(true);

        controller.publish(payload("/repo/c.ts", 1), 1);
        expect(controller.selectProblems()).not.toBe(problems);
        expect(controller.selectDocument("/repo/a.ts")).not.toBe(editor);
    });

    it("publishes immutable snapshots and isolates failing snapshot listeners", () => {
        const controller = new DiagnosticsController("/repo");
        const listener = vi.fn();
        const broken = vi.fn(() => {
            throw new Error("listener failed");
        });
        const unsubscribe = controller.subscribe(listener);
        controller.subscribe(broken);

        controller.activateServer("typescript", 1);
        controller.publish(payload("/repo/a.ts", 1), 1);
        expect(listener).toHaveBeenCalledTimes(2);
        expect(broken).toHaveBeenCalledOnce();
        const snapshot = controller.getSnapshot();
        expect(snapshot).toMatchObject({ project: "/repo", revision: 2, activeServers: 1, documents: 1, problems: 1 });
        expect(Object.isFrozen(snapshot)).toBe(true);

        unsubscribe();
        unsubscribe();
        controller.clearProject();
        expect(listener).toHaveBeenCalledTimes(2);
    });
});

describe("DiagnosticsController validation and disposal", () => {
    it("rejects unsafe construction and lifecycle identifiers", () => {
        expect(() => new DiagnosticsController(" ")).toThrow(TypeError);
        expect(() => new DiagnosticsController("/bad\nproject")).toThrow(TypeError);
        expect(() => new DiagnosticsController("x".repeat(LSP_PAYLOAD_LIMITS.maxPathBytes + 1))).toThrow(TypeError);

        const controller = new DiagnosticsController("/repo");
        expect(() => controller.activateServer("bad\nlanguage", 1)).toThrow(TypeError);
        expect(() => controller.activateServer("typescript", 0)).toThrow(RangeError);
        expect(() => controller.clearDocument("bad\npath")).toThrow(TypeError);
        for (let index = 0; index < DIAGNOSTICS_CONTROLLER_LIMITS.maxServers; index += 1) {
            controller.activateServer(`language${index}`, 1);
        }
        expect(() => controller.activateServer("overflow", 1)).toThrow(RangeError);

        controller.dispose();
        expect(() => controller.activateServer("typescript", 2)).toThrow("disposed");
        expect(() => controller.subscribe(() => {})).toThrow("disposed");
        expect(controller.clearDocument("/repo/a.ts")).toBe(0);
        expect(controller.clearProject()).toBe(0);
        expect(controller.shutdownServer("typescript", 1)).toBe(false);
    });
});
