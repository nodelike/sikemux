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
    documentSymbols: (project: string, language: string, path: string) =>
        invoke<LspDocumentSymbol[]>("lsp_document_symbols", { project, language, path }),
    subscribeDiagnostics: (listener: LspDiagnosticsListener, options: LspDiagnosticsSubscriptionOptions = {}): Promise<IpcUnsubscribe> =>
        getIpcTransport().subscribe<LspDiagnosticsPayload>(LSP_DIAGNOSTICS_EVENT, (event) => listener(event.payload), options),
};
