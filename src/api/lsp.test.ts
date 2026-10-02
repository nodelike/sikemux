import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { performanceTelemetry } from "../lib/performance";

import {
    LSP_DIAGNOSTICS_EVENT,
    LSP_PAYLOAD_LIMITS,
    documentLanguageIdFromPath,
    languageFromPath,
    lsp,
    parseLspDiagnosticsPayload,
    parseLspDocumentSymbols,
    uriToPath,
    type LspDiagnosticsPayload,
} from "./lsp";
import { MemoryIpcTransport, installIpcTransportForTests, resetIpcTransportForTests } from "./transport";

const position = (line = 1, character = 2) => ({ line, character });
const range = (start = position(), end = position(3, 4)) => ({ start, end });

function diagnostic(overrides: Record<string, unknown> = {}): Record<string, unknown> {
    return {
        range: range(),
        severity: "warning",
        code: "W100",
        source: "typescript",
        message: "Something needs attention",
        ...overrides,
    };
}

function diagnosticsPayload(overrides: Record<string, unknown> = {}): Record<string, unknown> {
    return {
        project: "/workspace/project",
        language: "typescript",
        path: "/workspace/project/main.ts",
        version: 7,
        diagnostics: [diagnostic()],
        ...overrides,
    };
}

function symbol(name: string, children: unknown[] = [], overrides: Record<string, unknown> = {}): Record<string, unknown> {
    return {
        name,
        detail: null,
        kind: 12,
        range: range(),
        selectionRange: range(position(1, 2), position(1, 8)),
        children,
        ...overrides,
    };
}

let transport: MemoryIpcTransport;

beforeEach(() => {
    resetIpcTransportForTests();
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    performanceTelemetry.reset();
});

afterEach(() => {
    resetIpcTransportForTests();
});

describe("existing LSP API", () => {
    it("preserves language detection and URI conversion", () => {
        expect(languageFromPath("/project/file.tsx")).toBe("typescript");
        expect(documentLanguageIdFromPath("/project/file.tsx")).toBe("typescriptreact");
        expect(languageFromPath("/project/file.unknown")).toBeNull();
        expect(uriToPath("file:///project/a%20file.ts")).toBe("/project/a file.ts");
        expect(uriToPath("https://example.test/file.ts")).toBe("https://example.test/file.ts");
    });
});

describe("LSP diagnostics subscription", () => {
    it("routes memory-transport events into immutable typed replacements and clears", async () => {
        const listener = vi.fn<(payload: LspDiagnosticsPayload) => void>();

        const unsubscribe = await lsp.subscribeDiagnostics(listener);
        expect(transport.eventListenerCount).toBe(1);

        const published = diagnosticsPayload();
        expect(transport.emit(LSP_DIAGNOSTICS_EVENT, published)).toEqual({ delivered: 1, listenerErrors: 0 });
        transport.emit(LSP_DIAGNOSTICS_EVENT, diagnosticsPayload({ version: 6 }));
        transport.emit(LSP_DIAGNOSTICS_EVENT, diagnosticsPayload({ version: null, diagnostics: [] }));

        expect(listener).toHaveBeenCalledTimes(3);
        expect(listener.mock.calls.map(([payload]) => payload.version)).toEqual([7, 6, null]);
        expect(listener.mock.calls[2][0].diagnostics).toEqual([]);
        const received = listener.mock.calls[0][0];
        expect(received).toEqual(published);
        expect(Object.isFrozen(received)).toBe(true);
        expect(Object.isFrozen(received.diagnostics)).toBe(true);
        expect(Object.isFrozen(received.diagnostics[0].range.start)).toBe(true);

        unsubscribe();
        unsubscribe();
        expect(transport.eventListenerCount).toBe(0);
        expect(transport.emit(LSP_DIAGNOSTICS_EVENT, diagnosticsPayload())).toEqual({ delivered: 0, listenerErrors: 0 });
        expect(listener).toHaveBeenCalledTimes(3);
    });

    it("ignores malformed payloads atomically without blocking later valid events", async () => {
        const listener = vi.fn();
        await lsp.subscribeDiagnostics(listener);

        const invalidPayloads: unknown[] = [
            null,
            diagnosticsPayload({ version: Number.MAX_SAFE_INTEGER + 1 }),
            diagnosticsPayload({ diagnostics: Array.from({ length: LSP_PAYLOAD_LIMITS.maxDiagnostics + 1 }, () => diagnostic()) }),
            diagnosticsPayload({ diagnostics: [diagnostic({ severity: "fatal" })] }),
            diagnosticsPayload({ diagnostics: [diagnostic({ message: "é".repeat(1_025) })] }),
            diagnosticsPayload({ diagnostics: [diagnostic({ code: 42 })] }),
            diagnosticsPayload({ diagnostics: [diagnostic({ range: range(position(3, 0), position(2, 0)) })] }),
            Object.defineProperty({}, "project", { enumerable: true, get: () => "/unsafe" }),
        ];
        for (const payload of invalidPayloads) transport.emit(LSP_DIAGNOSTICS_EVENT, payload);
        expect(listener).not.toHaveBeenCalled();

        transport.emit(LSP_DIAGNOSTICS_EVENT, diagnosticsPayload({ diagnostics: [] }));
        expect(listener).toHaveBeenCalledOnce();
    });

    it("validates exact UTF-8 limits and nullable fields", () => {
        const atLimit = diagnosticsPayload({
            diagnostics: [
                diagnostic({
                    severity: null,
                    code: null,
                    source: "é".repeat(LSP_PAYLOAD_LIMITS.maxDiagnosticSourceBytes / 2),
                    message: "é".repeat(LSP_PAYLOAD_LIMITS.maxDiagnosticMessageBytes / 2),
                }),
            ],
        });
        expect(parseLspDiagnosticsPayload(atLimit)).toMatchObject({ diagnostics: [{ severity: null, code: null }] });

        expect(
            parseLspDiagnosticsPayload(
                diagnosticsPayload({
                    diagnostics: [diagnostic({ source: `${"é".repeat(LSP_PAYLOAD_LIMITS.maxDiagnosticSourceBytes / 2)}é` })],
                }),
            ),
        ).toBeNull();
        expect(
            parseLspDiagnosticsPayload(
                diagnosticsPayload({ diagnostics: [diagnostic({ message: `${"é".repeat(LSP_PAYLOAD_LIMITS.maxDiagnosticMessageBytes / 2)}é` })] }),
            ),
        ).toBeNull();
    });

    it.each([
        [
            "missing version",
            (() => {
                const payload = diagnosticsPayload();
                delete payload.version;
                return payload;
            })(),
        ],
        ["oversized project", diagnosticsPayload({ project: "p".repeat(LSP_PAYLOAD_LIMITS.maxPathBytes + 1) })],
        ["control-bearing path", diagnosticsPayload({ path: "/project/bad\nfile.ts" })],
        ["oversized language", diagnosticsPayload({ language: "l".repeat(LSP_PAYLOAD_LIMITS.maxLanguageBytes + 1) })],
        ["control-bearing language", diagnosticsPayload({ language: "type\nscript" })],
        ["negative position", diagnosticsPayload({ diagnostics: [diagnostic({ range: range(position(-1, 0), position()) })] })],
        ["fractional position", diagnosticsPayload({ diagnostics: [diagnostic({ range: range(position(1.5, 0), position()) })] })],
        ["oversized position", diagnosticsPayload({ diagnostics: [diagnostic({ range: range(position(0, 0), position(0x1_0000_0000, 0)) })] })],
        ["missing diagnostic field", diagnosticsPayload({ diagnostics: [{ range: range(), message: "missing option fields" }] })],
    ])("rejects malformed diagnostics payload: %s", (_label, payload) => {
        expect(parseLspDiagnosticsPayload(payload)).toBeNull();
    });

    it("rejects non-function listeners before installing a global subscription", () => {
        expect(() => lsp.subscribeDiagnostics("listener" as never)).toThrow(TypeError);
        expect(transport.eventListenerCount).toBe(0);
    });

    it("cancels a diagnostics subscription through the shared transport signal", async () => {
        const controller = new AbortController();
        const listener = vi.fn();
        const unsubscribe = await lsp.subscribeDiagnostics(listener, { signal: controller.signal });
        expect(transport.eventListenerCount).toBe(1);

        controller.abort(new Error("project closed"));
        unsubscribe();
        expect(transport.eventListenerCount).toBe(0);
        transport.emit(LSP_DIAGNOSTICS_EVENT, diagnosticsPayload());
        expect(listener).not.toHaveBeenCalled();
    });
});

describe("LSP document symbols", () => {
    it("requests, validates, clones, and freezes hierarchical symbols", async () => {
        const response = [
            symbol("Workspace", [
                symbol("run", [], {
                    detail: "async function",
                    kind: 12,
                }),
            ]),
        ];
        const handler = vi.fn((_args: unknown) => response);
        transport.register("lsp_document_symbols", handler);

        const symbols = await lsp.documentSymbols("/project", "typescript", "/project/main.ts");

        expect(handler.mock.calls[0]![0]).toEqual({
            project: "/project",
            language: "typescript",
            path: "/project/main.ts",
        });
        expect(symbols).toEqual(response);
        expect(symbols).not.toBe(response);
        expect(Object.isFrozen(symbols)).toBe(true);
        expect(Object.isFrozen(symbols[0])).toBe(true);
        expect(Object.isFrozen(symbols[0].children)).toBe(true);
        expect(Object.isFrozen(symbols[0].children[0].selectionRange.end)).toBe(true);
    });

    it("accepts the native depth, count, and UTF-8 string boundaries", () => {
        let nested: Record<string, unknown> = symbol("leaf");
        for (let depth = 1; depth < LSP_PAYLOAD_LIMITS.maxDocumentSymbolDepth; depth += 1) {
            nested = symbol(`level${depth}`, [nested]);
        }
        const many = Array.from({ length: LSP_PAYLOAD_LIMITS.maxDocumentSymbols }, (_value, index) => symbol(`symbol${index}`));
        const boundedStrings = [
            symbol("é".repeat(LSP_PAYLOAD_LIMITS.maxSymbolNameBytes / 2), [], {
                detail: "é".repeat(LSP_PAYLOAD_LIMITS.maxSymbolDetailBytes / 2),
            }),
        ];

        expect(parseLspDocumentSymbols([nested])).not.toBeNull();
        expect(parseLspDocumentSymbols(many)).toHaveLength(LSP_PAYLOAD_LIMITS.maxDocumentSymbols);
        expect(parseLspDocumentSymbols(boundedStrings)).not.toBeNull();
    });

    it.each([
        ["non-array response", {}],
        ["too many symbols", Array.from({ length: LSP_PAYLOAD_LIMITS.maxDocumentSymbols + 1 }, () => symbol("symbol"))],
        ["oversized name", [symbol("é".repeat(LSP_PAYLOAD_LIMITS.maxSymbolNameBytes / 2 + 1))]],
        ["oversized detail", [symbol("name", [], { detail: "é".repeat(LSP_PAYLOAD_LIMITS.maxSymbolDetailBytes / 2 + 1) })]],
        ["negative kind", [symbol("name", [], { kind: -1 })]],
        ["fractional kind", [symbol("name", [], { kind: 1.5 })]],
        ["oversized kind", [symbol("name", [], { kind: 0x1_0000_0000 })]],
        [
            "missing selection range",
            [
                (() => {
                    const value = symbol("name");
                    delete value.selectionRange;
                    return value;
                })(),
            ],
        ],
        ["non-array children", [symbol("name", [], { children: {} })]],
        ["reversed range", [symbol("name", [], { range: range(position(8, 0), position(2, 0)) })]],
        ["accessor field", [Object.defineProperty({}, "name", { enumerable: true, get: () => "unsafe" })]],
    ])("rejects malformed document symbols: %s", (_label, payload) => {
        expect(parseLspDocumentSymbols(payload)).toBeNull();
    });

    it("rejects trees beyond the native depth and total-node limits", () => {
        let tooDeep: Record<string, unknown> = symbol("leaf");
        for (let depth = 0; depth < LSP_PAYLOAD_LIMITS.maxDocumentSymbolDepth; depth += 1) {
            tooDeep = symbol(`level${depth}`, [tooDeep]);
        }
        const tooWide = [
            symbol(
                "root",
                Array.from({ length: LSP_PAYLOAD_LIMITS.maxDocumentSymbols }, () => symbol("child")),
            ),
        ];

        expect(parseLspDocumentSymbols([tooDeep])).toBeNull();
        expect(parseLspDocumentSymbols(tooWide)).toBeNull();
    });

    it("rejects malformed IPC responses while preserving native errors", async () => {
        const nativeError = { category: "lsp", message: "server not started" };
        const handler = vi
            .fn()
            .mockReturnValueOnce([{ name: "missing fields" }])
            .mockRejectedValueOnce(nativeError);
        transport.register("lsp_document_symbols", handler);
        await expect(lsp.documentSymbols("/project", "rust", "/project/main.rs")).rejects.toThrow(TypeError);

        await expect(lsp.documentSymbols("/project", "rust", "/project/main.rs")).rejects.toBe(nativeError);
    });
});
