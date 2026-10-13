import { invokeCommand as invoke } from "./invoke";
import { fileUriToPath } from "../lib/paths";
import { languageServerOf } from "../languages";
import { getIpcTransport, type IpcUnsubscribe } from "./transport";

export interface LspPos {
    line: number;
    character: number;
}

export interface LspRange {
    start: LspPos;
    end: LspPos;
}

export interface LspLocation {
    uri: string;
    range: LspRange;
}

export type LspLocationKind = "definition" | "declaration" | "typeDefinition" | "implementation" | "references";

export interface LspTextChange {
    range?: LspRange;
    rangeLength?: number;
    text: string;
}

export type LspDiagnosticSeverity = "error" | "warning" | "information" | "hint";

export interface LspDiagnostic {
    readonly range: LspRange;
    readonly severity: LspDiagnosticSeverity | null;
    readonly code: string | null;
    readonly source: string | null;
    readonly message: string;
}

export type LspDocumentVersion = number | null;

export interface LspDiagnosticsPayload {
    readonly project: string;
    readonly language: string;
    readonly path: string;
    /** `null` means the server omitted a version; consumers decide freshness. */
    readonly version: LspDocumentVersion;
    readonly diagnostics: readonly LspDiagnostic[];
}

export interface LspDocumentSymbol {
    readonly name: string;
    readonly detail: string | null;
    readonly kind: number;
    readonly range: LspRange;
    readonly selectionRange: LspRange;
    readonly children: readonly LspDocumentSymbol[];
}

export type LspDiagnosticsListener = (payload: LspDiagnosticsPayload) => void;

export interface LspDiagnosticsSubscriptionOptions {
    readonly signal?: AbortSignal;
}

export const LSP_DIAGNOSTICS_EVENT = "lsp_diagnostics";

/** Mirrors the native payload caps at the IPC boundary. */
export const LSP_PAYLOAD_LIMITS = Object.freeze({
    maxDiagnostics: 500,
    maxDiagnosticMessageBytes: 2_048,
    maxDiagnosticSourceBytes: 128,
    maxDiagnosticCodeBytes: 128,
    maxPathBytes: 4_096,
    maxLanguageBytes: 128,
    maxDocumentSymbols: 2_000,
    maxDocumentSymbolDepth: 16,
    maxSymbolNameBytes: 256,
    maxSymbolDetailBytes: 1_024,
});

const INVALID_PROPERTY = Symbol("invalid LSP property");
const U32_MAX = 0xffff_ffff;
const UTF8_ENCODER = new TextEncoder();

function isRecord(value: unknown): value is Record<PropertyKey, unknown> {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

function ownDataProperty(value: Record<PropertyKey, unknown>, key: string): unknown | typeof INVALID_PROPERTY {
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    return descriptor && "value" in descriptor ? descriptor.value : INVALID_PROPERTY;
}

function boundedString(value: unknown, maxBytes: number, allowEmpty = true): string | null {
    if (typeof value !== "string" || (!allowEmpty && value.length === 0) || value.length > maxBytes) return null;
    // UTF-8 never spends more than three bytes per UTF-16 unit, so a short
    // enough string is within budget without encoding it to find out.
    if (value.length * 3 <= maxBytes) return value;
    return UTF8_ENCODER.encode(value).byteLength <= maxBytes ? value : null;
}

function containsControlCharacter(value: string): boolean {
    for (let index = 0; index < value.length; index += 1) {
        const code = value.charCodeAt(index);
        if (code <= 31 || (code >= 127 && code <= 159)) return true;
    }
    return false;
}

function boundedPath(value: unknown): string | null {
    const path = boundedString(value, LSP_PAYLOAD_LIMITS.maxPathBytes, false);
    return path !== null && path.trim().length > 0 && !containsControlCharacter(path) ? path : null;
}

function boundedLanguage(value: unknown): string | null {
    const language = boundedString(value, LSP_PAYLOAD_LIMITS.maxLanguageBytes, false);
    return language !== null && language === language.trim() && !containsControlCharacter(language) ? language : null;
}

function readPosition(value: unknown): LspPos | null {
    if (!isRecord(value)) return null;
    const line = ownDataProperty(value, "line");
    const character = ownDataProperty(value, "character");
    if (
        typeof line !== "number" ||
        !Number.isInteger(line) ||
        line < 0 ||
        line > U32_MAX ||
        typeof character !== "number" ||
        !Number.isInteger(character) ||
        character < 0 ||
        character > U32_MAX
    ) {
        return null;
    }
    return Object.freeze({ line, character });
}

function positionAtOrBefore(left: LspPos, right: LspPos): boolean {
    return left.line < right.line || (left.line === right.line && left.character <= right.character);
}

function readRange(value: unknown): LspRange | null {
    if (!isRecord(value)) return null;
    const start = readPosition(ownDataProperty(value, "start"));
    const end = readPosition(ownDataProperty(value, "end"));
    if (!start || !end || !positionAtOrBefore(start, end)) return null;
    return Object.freeze({ start, end });
}

function readNullableBoundedString(value: unknown, maxBytes: number): string | null | typeof INVALID_PROPERTY {
    if (value === null) return null;
    return boundedString(value, maxBytes) ?? INVALID_PROPERTY;
}

function readDiagnostic(value: unknown): LspDiagnostic | null {
    if (!isRecord(value)) return null;
    const range = readRange(ownDataProperty(value, "range"));
    const severityValue = ownDataProperty(value, "severity");
    const code = readNullableBoundedString(ownDataProperty(value, "code"), LSP_PAYLOAD_LIMITS.maxDiagnosticCodeBytes);
    const source = readNullableBoundedString(ownDataProperty(value, "source"), LSP_PAYLOAD_LIMITS.maxDiagnosticSourceBytes);
    const message = boundedString(ownDataProperty(value, "message"), LSP_PAYLOAD_LIMITS.maxDiagnosticMessageBytes);
    const severity =
        severityValue === null ||
        severityValue === "error" ||
        severityValue === "warning" ||
        severityValue === "information" ||
        severityValue === "hint"
            ? severityValue
            : INVALID_PROPERTY;
    if (!range || severity === INVALID_PROPERTY || code === INVALID_PROPERTY || source === INVALID_PROPERTY || message === null) {
        return null;
    }
    return Object.freeze({ range, severity, code, source, message });
}

function readDiagnosticsPayload(value: unknown): LspDiagnosticsPayload | null {
    if (!isRecord(value)) return null;
    const project = boundedPath(ownDataProperty(value, "project"));
    const language = boundedLanguage(ownDataProperty(value, "language"));
    const path = boundedPath(ownDataProperty(value, "path"));
    const versionValue = ownDataProperty(value, "version");
    const diagnosticsValue = ownDataProperty(value, "diagnostics");
    const version =
        versionValue === null || (typeof versionValue === "number" && Number.isSafeInteger(versionValue)) ? versionValue : INVALID_PROPERTY;
    if (
        project === null ||
        language === null ||
        path === null ||
        version === INVALID_PROPERTY ||
        !Array.isArray(diagnosticsValue) ||
        diagnosticsValue.length > LSP_PAYLOAD_LIMITS.maxDiagnostics
    ) {
        return null;
    }
    const diagnostics: LspDiagnostic[] = [];
    for (const diagnosticValue of diagnosticsValue) {
        const diagnostic = readDiagnostic(diagnosticValue);
        if (!diagnostic) return null;
        diagnostics.push(diagnostic);
    }
    return Object.freeze({ project, language, path, version, diagnostics: Object.freeze(diagnostics) });
}

export function parseLspDiagnosticsPayload(value: unknown): LspDiagnosticsPayload | null {
    try {
        return readDiagnosticsPayload(value);
    } catch {
        return null;
    }
}

type SymbolParseState = { count: number };

function readDocumentSymbol(value: unknown, depth: number, state: SymbolParseState): LspDocumentSymbol | null {
    if (!isRecord(value) || depth > LSP_PAYLOAD_LIMITS.maxDocumentSymbolDepth) return null;
    state.count += 1;
    if (state.count > LSP_PAYLOAD_LIMITS.maxDocumentSymbols) return null;

    const name = boundedString(ownDataProperty(value, "name"), LSP_PAYLOAD_LIMITS.maxSymbolNameBytes);
    const detail = readNullableBoundedString(ownDataProperty(value, "detail"), LSP_PAYLOAD_LIMITS.maxSymbolDetailBytes);
    const kind = ownDataProperty(value, "kind");
    const range = readRange(ownDataProperty(value, "range"));
    const selectionRange = readRange(ownDataProperty(value, "selectionRange"));
    const childrenValue = ownDataProperty(value, "children");
    if (
        name === null ||
        detail === INVALID_PROPERTY ||
        typeof kind !== "number" ||
        !Number.isInteger(kind) ||
        kind < 0 ||
        kind > U32_MAX ||
        !range ||
        !selectionRange ||
        !Array.isArray(childrenValue) ||
        childrenValue.length > LSP_PAYLOAD_LIMITS.maxDocumentSymbols - state.count
    ) {
        return null;
    }

    const children: LspDocumentSymbol[] = [];
    for (const childValue of childrenValue) {
        const child = readDocumentSymbol(childValue, depth + 1, state);
        if (!child) return null;
        children.push(child);
    }
    return Object.freeze({ name, detail, kind, range, selectionRange, children: Object.freeze(children) });
}

function readDocumentSymbols(value: unknown): readonly LspDocumentSymbol[] | null {
    if (!Array.isArray(value) || value.length > LSP_PAYLOAD_LIMITS.maxDocumentSymbols) return null;
    const state: SymbolParseState = { count: 0 };
    const symbols: LspDocumentSymbol[] = [];
    for (const symbolValue of value) {
        const symbol = readDocumentSymbol(symbolValue, 1, state);
        if (!symbol) return null;
        symbols.push(symbol);
    }
    return Object.freeze(symbols);
}

export function parseLspDocumentSymbols(value: unknown): readonly LspDocumentSymbol[] | null {
    try {
        return readDocumentSymbols(value);
    } catch {
        return null;
    }
}

let servedLanguages: ReadonlySet<string> = new Set();
let servedLanguagesAsked: Promise<void> | null = null;

export function noteServedLanguages(languages: readonly string[]): void {
    servedLanguages = new Set(languages);
}

/** Asks the native side once which languages a server can start for, including any a `SIKEMUX_LSP_<LANG>` variable adds. */
export function loadServedLanguages(): Promise<void> {
    servedLanguagesAsked ??= invoke<string[]>("lsp_languages").then(noteServedLanguages, () => {
        servedLanguagesAsked = null;
    });
    return servedLanguagesAsked;
}

/** The language server a file is opened with, when one can start for its language. */
export function languageFromPath(path: string): string | null {
    const language = languageServerOf(path);
    return language && servedLanguages.has(language) ? language : null;
}

export const uriToPath = (uri: string): string => fileUriToPath(uri) ?? uri;

export const lsp = {
    start: (project: string, language: string) => invoke<void>("lsp_start", { project, language }),
    stop: (project: string) => invoke<void>("lsp_stop", { project }),
    install: (language: string) => invoke<string>("lsp_install_server", { language }),
    open: (project: string, language: string, path: string, content: string, languageId: string = language) =>
        invoke<void>("lsp_open", { project, language, path, content, languageId }),
    change: (project: string, language: string, path: string, content: string, version: number) =>
        invoke<void>("lsp_change", { project, language, path, content, version }),
    changeIncremental: (project: string, language: string, path: string, changes: LspTextChange[], version: number) =>
        invoke<void>("lsp_change_incremental", { project, language, path, changes, version }),
    save: (project: string, language: string, path: string, content?: string | null) =>
        invoke<void>("lsp_save", { project, language, path, content: content ?? null }),
    close: (project: string, language: string, path: string) => invoke<void>("lsp_close", { project, language, path }),
    locations: (project: string, language: string, path: string, line: number, character: number, kind: LspLocationKind) =>
        invoke<LspLocation[]>("lsp_locations", {
            project,
            language,
            path,
            line,
            character,
            kind,
        }),
    definition: (project: string, language: string, path: string, line: number, character: number) =>
        lsp.locations(project, language, path, line, character, "definition"),
    declaration: (project: string, language: string, path: string, line: number, character: number) =>
        lsp.locations(project, language, path, line, character, "declaration"),
    typeDefinition: (project: string, language: string, path: string, line: number, character: number) =>
        lsp.locations(project, language, path, line, character, "typeDefinition"),
    implementation: (project: string, language: string, path: string, line: number, character: number) =>
        lsp.locations(project, language, path, line, character, "implementation"),
    references: (project: string, language: string, path: string, line: number, character: number) =>
        lsp.locations(project, language, path, line, character, "references"),
    documentSymbols: async (project: string, language: string, path: string): Promise<readonly LspDocumentSymbol[]> => {
        const payload = await invoke<unknown>("lsp_document_symbols", { project, language, path });
        const symbols = parseLspDocumentSymbols(payload);
        if (!symbols) throw new TypeError("Invalid lsp_document_symbols response");
        return symbols;
    },
    subscribeDiagnostics: (listener: LspDiagnosticsListener, options: LspDiagnosticsSubscriptionOptions = {}): Promise<IpcUnsubscribe> => {
        if (typeof listener !== "function") throw new TypeError("LSP diagnostics listener must be a function");
        return getIpcTransport().subscribe<unknown>(
            LSP_DIAGNOSTICS_EVENT,
            (event) => {
                const payload = parseLspDiagnosticsPayload(event.payload);
                if (payload) listener(payload);
            },
            options,
        );
    },
};
