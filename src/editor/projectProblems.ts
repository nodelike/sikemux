import { lsp, type LspDiagnostic, type LspDiagnosticsPayload, type LspDocumentVersion, type LspPos } from "../api/lsp";
import { swallow } from "../state/toast";

export interface DiagnosticProblem {
    readonly language: string;
    readonly path: string;
    readonly range: LspDiagnostic["range"];
    readonly severity: LspDiagnostic["severity"];
    readonly code: string | null;
    readonly source: string | null;
    readonly message: string;
}

interface PublishedDocument {
    readonly language: string;
    readonly path: string;
    readonly version: LspDocumentVersion;
    readonly diagnostics: readonly LspDiagnostic[];
}

const SEVERITY_RANK: Record<NonNullable<LspDiagnostic["severity"]>, number> = { error: 0, warning: 1, information: 2, hint: 3 };
const NO_PROBLEMS: readonly DiagnosticProblem[] = Object.freeze([]);

const severityRank = (severity: LspDiagnostic["severity"]): number => (severity ? SEVERITY_RANK[severity] : 4);
const comparePosition = (left: LspPos, right: LspPos): number => left.line - right.line || left.character - right.character;
const compareText = (left: string | null, right: string | null): number => (left ?? "").localeCompare(right ?? "");

function compareProblem(left: DiagnosticProblem, right: DiagnosticProblem): number {
    return (
        severityRank(left.severity) - severityRank(right.severity) ||
        compareText(left.path, right.path) ||
        comparePosition(left.range.start, right.range.start) ||
        comparePosition(left.range.end, right.range.end) ||
        compareText(left.language, right.language) ||
        compareText(left.source, right.source) ||
        compareText(left.code, right.code) ||
        compareText(left.message, right.message)
    );
}

/** The language servers' latest diagnostics for one project, from the servers started since it opened. */
export class ProjectProblems {
    private readonly documents = new Map<string, PublishedDocument>();
    private readonly languages = new Set<string>();
    private readonly listeners = new Set<() => void>();
    private sorted: readonly DiagnosticProblem[] | null = NO_PROBLEMS;
    references = 0;

    constructor(readonly project: string) {}

    subscribe = (listener: () => void): (() => void) => {
        this.listeners.add(listener);
        return () => this.listeners.delete(listener);
    };

    problems = (): readonly DiagnosticProblem[] => {
        if (this.sorted) return this.sorted;
        const problems: DiagnosticProblem[] = [];
        for (const { language, path, diagnostics } of this.documents.values()) {
            for (const { range, severity, code, source, message } of diagnostics)
                problems.push({ language, path, range, severity, code, source, message });
        }
        this.sorted = problems.length ? problems.sort(compareProblem) : NO_PROBLEMS;
        return this.sorted;
    };

    serverStarted(language: string): void {
        if (this.languages.has(language)) return;
        this.languages.add(language);
        for (const [key, document] of this.documents) if (document.language === language) this.documents.delete(key);
        this.changed();
    }

    publish(payload: LspDiagnosticsPayload): void {
        if (!this.languages.has(payload.language)) return;
        const key = `${payload.language}\0${payload.path}`;
        const current = this.documents.get(key);
        if (current?.version != null && (payload.version === null || payload.version < current.version)) return;
        this.documents.set(key, payload);
        this.changed();
    }

    private changed(): void {
        this.sorted = null;
        for (const listener of this.listeners) listener();
    }
}

const projects = new Map<string, ProjectProblems>();
let stopListening: Promise<() => void> | null = null;

function route(payload: LspDiagnosticsPayload): void {
    projects.get(payload.project)?.publish(payload);
}

/** Shares one project's problems with every editor open in it; release when the editor closes. */
export function acquireProjectProblems(project: string): { readonly problems: ProjectProblems; release(): void } {
    let problems = projects.get(project);
    if (!problems) {
        problems = new ProjectProblems(project);
        projects.set(project, problems);
    }
    problems.references += 1;
    if (!stopListening) {
        const listening = lsp.subscribeDiagnostics(route);
        stopListening = listening;
        listening.catch((error: unknown) => {
            if (stopListening === listening) stopListening = null;
            swallow("LSP diagnostics listener")(error);
        });
    }
    const held = problems;
    let released = false;
    return {
        problems: held,
        release: () => {
            if (released) return;
            released = true;
            held.references -= 1;
            if (held.references > 0) return;
            projects.delete(project);
            if (projects.size > 0 || !stopListening) return;
            void stopListening.then((stop) => stop()).catch(swallow("LSP diagnostics listener"));
            stopListening = null;
        },
    };
}

export function noteServerStarted(project: string, language: string): void {
    projects.get(project)?.serverStarted(language);
}
