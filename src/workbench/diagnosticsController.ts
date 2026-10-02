import {
    LSP_PAYLOAD_LIMITS,
    parseLspDiagnosticsPayload,
    type LspDiagnostic,
    type LspDiagnosticsPayload,
    type LspDocumentVersion,
    type LspPos,
} from "../api/lsp";

export type DiagnosticsServerGeneration = number;

export type DiagnosticsPublishResult =
    | "applied"
    | "disposed"
    | "invalid-payload"
    | "invalid-generation"
    | "wrong-project"
    | "generation-mismatch"
    | "stale-version"
    | "unversioned-after-numbered"
    | "capacity";

export interface DiagnosticProblem {
    readonly project: string;
    readonly language: string;
    readonly path: string;
    readonly serverGeneration: DiagnosticsServerGeneration;
    readonly version: LspDocumentVersion;
    readonly range: LspDiagnostic["range"];
    readonly severity: LspDiagnostic["severity"];
    readonly code: string | null;
    readonly source: string | null;
    readonly message: string;
}

export interface DiagnosticsDocumentView {
    readonly project: string;
    readonly language: string;
    readonly path: string;
    readonly serverGeneration: DiagnosticsServerGeneration;
    readonly version: LspDocumentVersion;
    readonly diagnostics: readonly LspDiagnostic[];
}

export interface DiagnosticsControllerSnapshot {
    readonly project: string;
    readonly revision: number;
    readonly activeServers: number;
    readonly documents: number;
    readonly problems: number;
    readonly disposed: boolean;
}

export const DIAGNOSTICS_CONTROLLER_LIMITS = Object.freeze({
    maxServers: 32,
    maxDocuments: 2_048,
    maxStoredDiagnostics: 10_000,
    maxListeners: 64,
});

type SnapshotListener = () => void;

type ServerState = {
    readonly generation: DiagnosticsServerGeneration;
    readonly active: boolean;
};

type StoredDocument = {
    readonly key: string;
    readonly project: string;
    readonly language: string;
    readonly path: string;
    readonly serverGeneration: DiagnosticsServerGeneration;
    readonly version: LspDocumentVersion;
    readonly diagnostics: readonly LspDiagnostic[];
};

const UTF8_ENCODER = new TextEncoder();
const EMPTY_DIAGNOSTICS = Object.freeze([]) as readonly LspDiagnostic[];
const EMPTY_PROBLEMS = Object.freeze([]) as readonly DiagnosticProblem[];
const EMPTY_DOCUMENTS = Object.freeze([]) as readonly DiagnosticsDocumentView[];

function containsControlCharacter(value: string): boolean {
    for (let index = 0; index < value.length; index += 1) {
        const code = value.charCodeAt(index);
        if (code <= 31 || (code >= 127 && code <= 159)) return true;
    }
    return false;
}

function boundedUtf8(value: string, maxBytes: number): boolean {
    return value.length <= maxBytes && UTF8_ENCODER.encode(value).byteLength <= maxBytes;
}

function requireProject(value: string): string {
    if (
        typeof value !== "string" ||
        value.trim().length === 0 ||
        !boundedUtf8(value, LSP_PAYLOAD_LIMITS.maxPathBytes) ||
        containsControlCharacter(value)
    ) {
        throw new TypeError("diagnostics project must be a bounded non-blank path without control characters");
    }
    return value;
}

function requireLanguage(value: string): string {
    if (
        typeof value !== "string" ||
        value.length === 0 ||
        value !== value.trim() ||
        !boundedUtf8(value, LSP_PAYLOAD_LIMITS.maxLanguageBytes) ||
        containsControlCharacter(value)
    ) {
        throw new TypeError("diagnostics language must be bounded, trimmed text without control characters");
    }
    return value;
}

function requirePath(value: string): string {
    if (
        typeof value !== "string" ||
        value.trim().length === 0 ||
        !boundedUtf8(value, LSP_PAYLOAD_LIMITS.maxPathBytes) ||
        containsControlCharacter(value)
    ) {
        throw new TypeError("diagnostics path must be bounded non-blank text without control characters");
    }
    return value;
}

function isValidGeneration(value: number): boolean {
    return Number.isSafeInteger(value) && value > 0;
}

function requireGeneration(value: number): DiagnosticsServerGeneration {
    if (!isValidGeneration(value)) throw new RangeError("diagnostics server generation must be a positive safe integer");
    return value;
}

function documentKey(project: string, language: string, path: string, generation: DiagnosticsServerGeneration): string {
    return JSON.stringify([project, language, path, generation]);
}

function compareText(left: string, right: string): number {
    return left < right ? -1 : left > right ? 1 : 0;
}

function comparePosition(left: LspPos, right: LspPos): number {
    return left.line - right.line || left.character - right.character;
}

function severityRank(severity: LspDiagnostic["severity"]): number {
    switch (severity) {
        case "error":
            return 0;
        case "warning":
            return 1;
        case "information":
            return 2;
        case "hint":
            return 3;
        case null:
            return 4;
    }
}

function compareDiagnostic(left: LspDiagnostic, right: LspDiagnostic): number {
    return (
        comparePosition(left.range.start, right.range.start) ||
        comparePosition(left.range.end, right.range.end) ||
        severityRank(left.severity) - severityRank(right.severity) ||
        compareText(left.source ?? "", right.source ?? "") ||
        compareText(left.code ?? "", right.code ?? "") ||
        compareText(left.message, right.message)
    );
}

function compareProblem(left: DiagnosticProblem, right: DiagnosticProblem): number {
    return (
        severityRank(left.severity) - severityRank(right.severity) ||
        compareText(left.path, right.path) ||
        comparePosition(left.range.start, right.range.start) ||
        comparePosition(left.range.end, right.range.end) ||
        compareText(left.language, right.language) ||
        left.serverGeneration - right.serverGeneration ||
        compareText(left.source ?? "", right.source ?? "") ||
        compareText(left.code ?? "", right.code ?? "") ||
        compareText(left.message, right.message)
    );
}

function once(callback: () => void): () => void {
    let called = false;
    return () => {
        if (called) return;
        called = true;
        callback();
    };
}

/** Project-scoped owner for bounded, generation-aware LSP diagnostics. */
export class DiagnosticsController {
    private readonly servers = new Map<string, ServerState>();
    private readonly documents = new Map<string, StoredDocument>();
    private readonly listeners = new Set<SnapshotListener>();
    private readonly documentProblemCache = new Map<string, readonly DiagnosticProblem[]>();
    private totalDiagnostics = 0;
    private revision = 0;
    private problemsCache: readonly DiagnosticProblem[] | null = null;
    private documentsCache: readonly DiagnosticsDocumentView[] | null = null;
    private disposed = false;

    constructor(readonly project: string) {
        requireProject(project);
    }

    getSnapshot = (): DiagnosticsControllerSnapshot =>
        Object.freeze({
            project: this.project,
            revision: this.revision,
            activeServers: Array.from(this.servers.values()).filter((server) => server.active).length,
            documents: Array.from(this.documents.values()).filter((document) => document.diagnostics.length > 0).length,
            problems: this.totalDiagnostics,
            disposed: this.disposed,
        });

    subscribe = (listener: SnapshotListener): (() => void) => {
        if (this.disposed) throw new Error("cannot subscribe to a disposed diagnostics controller");
        if (typeof listener !== "function") throw new TypeError("diagnostics snapshot listener must be a function");
        if (!this.listeners.has(listener) && this.listeners.size >= DIAGNOSTICS_CONTROLLER_LIMITS.maxListeners) {
            throw new RangeError(`diagnostics controller cannot exceed ${DIAGNOSTICS_CONTROLLER_LIMITS.maxListeners} listeners`);
        }
        this.listeners.add(listener);
        return once(() => this.listeners.delete(listener));
    };

    activateServer(languageInput: string, generationInput: DiagnosticsServerGeneration): boolean {
        if (this.disposed) throw new Error("cannot activate a server on a disposed diagnostics controller");
        const language = requireLanguage(languageInput);
        const generation = requireGeneration(generationInput);
        const current = this.servers.get(language);
        if (current && generation <= current.generation) return false;
        if (!current && this.servers.size >= DIAGNOSTICS_CONTROLLER_LIMITS.maxServers) {
            throw new RangeError(`diagnostics controller cannot exceed ${DIAGNOSTICS_CONTROLLER_LIMITS.maxServers} servers`);
        }

        this.removeDocuments((document) => document.language === language);
        this.servers.set(language, Object.freeze({ generation, active: true }));
        this.commitMutation();
        return true;
    }

    publish(payloadInput: LspDiagnosticsPayload, generation: DiagnosticsServerGeneration): DiagnosticsPublishResult {
        if (this.disposed) return "disposed";
        if (!isValidGeneration(generation)) return "invalid-generation";
        const payload = parseLspDiagnosticsPayload(payloadInput);
        if (!payload) return "invalid-payload";
        if (payload.project !== this.project) return "wrong-project";
        const server = this.servers.get(payload.language);
        if (!server?.active || server.generation !== generation) return "generation-mismatch";

        const key = documentKey(this.project, payload.language, payload.path, generation);
        const current = this.documents.get(key);
        if (current) {
            if (payload.version === null && current.version !== null) return "unversioned-after-numbered";
            if (payload.version !== null && current.version !== null && payload.version < current.version) return "stale-version";
        }

        const nextTotal = this.totalDiagnostics - (current?.diagnostics.length ?? 0) + payload.diagnostics.length;
        if (nextTotal > DIAGNOSTICS_CONTROLLER_LIMITS.maxStoredDiagnostics) return "capacity";
        if (!current && this.documents.size >= DIAGNOSTICS_CONTROLLER_LIMITS.maxDocuments) return "capacity";

        const document: StoredDocument = Object.freeze({
            key,
            project: this.project,
            language: payload.language,
            path: payload.path,
            serverGeneration: generation,
            version: payload.version,
            diagnostics: payload.diagnostics,
        });
        this.documents.set(key, document);
        this.totalDiagnostics = nextTotal;
        this.commitMutation();
        return "applied";
    }

    clearDocument(pathInput: string): number {
        if (this.disposed) return 0;
        const path = requirePath(pathInput);
        return this.clearMatchingDocuments((document) => document.path === path);
    }

    clearProject(): number {
        if (this.disposed) return 0;
        return this.clearMatchingDocuments(() => true);
    }

    shutdownServer(languageInput: string, generationInput: DiagnosticsServerGeneration): boolean {
        if (this.disposed) return false;
        const language = requireLanguage(languageInput);
        const generation = requireGeneration(generationInput);
        const server = this.servers.get(language);
        if (!server?.active || server.generation !== generation) return false;

        this.removeDocuments((document) => document.language === language && document.serverGeneration === generation);
        this.servers.set(language, Object.freeze({ generation, active: false }));
        this.commitMutation();
        return true;
    }

    selectProblems(): readonly DiagnosticProblem[] {
        if (this.problemsCache) return this.problemsCache;
        const problems: DiagnosticProblem[] = [];
        for (const document of this.documents.values()) {
            for (const diagnostic of document.diagnostics) problems.push(this.problem(document, diagnostic));
        }
        problems.sort(compareProblem);
        this.problemsCache = problems.length === 0 ? EMPTY_PROBLEMS : Object.freeze(problems);
        return this.problemsCache;
    }

    selectDocument(pathInput: string): readonly DiagnosticProblem[] {
        const path = requirePath(pathInput);
        const cached = this.documentProblemCache.get(path);
        if (cached) return cached;
        const problems: DiagnosticProblem[] = [];
        for (const document of this.documents.values()) {
            if (document.path !== path) continue;
            for (const diagnostic of document.diagnostics) problems.push(this.problem(document, diagnostic));
        }
        problems.sort(
            (left, right) =>
                comparePosition(left.range.start, right.range.start) ||
                comparePosition(left.range.end, right.range.end) ||
                severityRank(left.severity) - severityRank(right.severity) ||
                compareText(left.language, right.language) ||
                left.serverGeneration - right.serverGeneration ||
                compareText(left.message, right.message),
        );
        const result = problems.length === 0 ? EMPTY_PROBLEMS : Object.freeze(problems);
        this.documentProblemCache.set(path, result);
        return result;
    }

    selectDocuments(): readonly DiagnosticsDocumentView[] {
        if (this.documentsCache) return this.documentsCache;
        const documents: DiagnosticsDocumentView[] = [];
        for (const document of this.documents.values()) {
            if (document.diagnostics.length === 0) continue;
            documents.push(
                Object.freeze({
                    project: document.project,
                    language: document.language,
                    path: document.path,
                    serverGeneration: document.serverGeneration,
                    version: document.version,
                    diagnostics: Object.freeze(document.diagnostics.slice().sort(compareDiagnostic)),
                }),
            );
        }
        documents.sort(
            (left, right) =>
                compareText(left.path, right.path) || compareText(left.language, right.language) || left.serverGeneration - right.serverGeneration,
        );
        this.documentsCache = documents.length === 0 ? EMPTY_DOCUMENTS : Object.freeze(documents);
        return this.documentsCache;
    }

    dispose(): void {
        if (this.disposed) return;
        this.disposed = true;
        this.servers.clear();
        this.documents.clear();
        this.totalDiagnostics = 0;
        this.revision += 1;
        this.invalidateSelectors();
        this.listeners.clear();
    }

    private problem(document: StoredDocument, diagnostic: LspDiagnostic): DiagnosticProblem {
        return Object.freeze({
            project: document.project,
            language: document.language,
            path: document.path,
            serverGeneration: document.serverGeneration,
            version: document.version,
            range: diagnostic.range,
            severity: diagnostic.severity,
            code: diagnostic.code,
            source: diagnostic.source,
            message: diagnostic.message,
        });
    }

    private clearMatchingDocuments(predicate: (document: StoredDocument) => boolean): number {
        let cleared = 0;
        for (const [key, document] of this.documents) {
            if (!predicate(document) || document.diagnostics.length === 0) continue;
            this.totalDiagnostics -= document.diagnostics.length;
            this.documents.set(key, Object.freeze({ ...document, diagnostics: EMPTY_DIAGNOSTICS }));
            cleared += 1;
        }
        if (cleared > 0) this.commitMutation();
        return cleared;
    }

    private removeDocuments(predicate: (document: StoredDocument) => boolean): number {
        let removed = 0;
        for (const [key, document] of this.documents) {
            if (!predicate(document)) continue;
            this.documents.delete(key);
            this.totalDiagnostics -= document.diagnostics.length;
            removed += 1;
        }
        return removed;
    }

    private commitMutation(): void {
        this.revision += 1;
        this.invalidateSelectors();
        for (const listener of Array.from(this.listeners)) {
            try {
                listener();
            } catch {
                this.listeners.delete(listener);
            }
        }
    }

    private invalidateSelectors(): void {
        this.problemsCache = null;
        this.documentsCache = null;
        this.documentProblemCache.clear();
    }
}
