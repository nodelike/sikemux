import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { languageFromPath, lsp, type LspDocumentSymbol } from "../api/lsp";
import type { DiagnosticProblem, DiagnosticsController } from "../workbench/diagnosticsController";
import { VirtualPanelRows } from "../git/VirtualPanelRows";
import { TreeContextMenu } from "../rail/FileTree";
import { SendToAgentMenu } from "../agents/SendToAgentMenu";
import { sessionOfPane } from "../agents/agentTargets";
import { getState } from "../state/store";
import { copyText } from "../lib/clipboard";
import { basename } from "../lib/paths";
import { reportError } from "../state/toast";
import { problemDelivery, problemLocation } from "./problemDelivery";

export type EditorInsightsTab = "problems" | "outline";

interface EditorInsightsProps {
    project: string;
    path: string | null;
    controller: DiagnosticsController | null;
    visible: boolean;
    onNavigate: (path: string, line: number, column: number) => void;
    /** Its project's agents are the ones a problem can be sent to. */
    paneId?: string;
}

interface ProblemMenu {
    x: number;
    y: number;
    problem: DiagnosticProblem;
    sending: boolean;
    sessionId?: string | null;
}

interface FlatSymbol {
    readonly key: string;
    readonly depth: number;
    readonly symbol: LspDocumentSymbol;
}

type OutlineState =
    | { readonly key: string; readonly status: "idle"; readonly symbols: readonly LspDocumentSymbol[] }
    | { readonly key: string; readonly status: "loading"; readonly symbols: readonly LspDocumentSymbol[] }
    | { readonly key: string; readonly status: "ready"; readonly symbols: readonly LspDocumentSymbol[] }
    | { readonly key: string; readonly status: "error"; readonly symbols: readonly LspDocumentSymbol[] };

const EMPTY_OUTLINE: OutlineState = Object.freeze({ key: "", status: "idle", symbols: Object.freeze([]) });
const NOOP_SUBSCRIBE = () => () => {};
const ZERO_REVISION = () => 0;

const SYMBOL_KINDS: Readonly<Record<number, string>> = Object.freeze({
    1: "file",
    2: "module",
    3: "namespace",
    4: "package",
    5: "class",
    6: "method",
    7: "property",
    8: "field",
    9: "constructor",
    10: "enum",
    11: "interface",
    12: "function",
    13: "variable",
    14: "constant",
    15: "string",
    16: "number",
    17: "boolean",
    18: "array",
    19: "object",
    20: "key",
    21: "null",
    22: "enum member",
    23: "struct",
    24: "event",
    25: "operator",
    26: "type parameter",
});

function flattenSymbols(symbols: readonly LspDocumentSymbol[]): FlatSymbol[] {
    const flattened: FlatSymbol[] = [];
    const visit = (items: readonly LspDocumentSymbol[], depth: number, prefix: string) => {
        for (const [index, symbol] of items.entries()) {
            const key = `${prefix}.${index}:${symbol.selectionRange.start.line}:${symbol.selectionRange.start.character}:${symbol.name}`;
            flattened.push(Object.freeze({ key, depth, symbol }));
            visit(symbol.children, depth + 1, key);
        }
    };
    visit(symbols, 0, "symbol");
    return flattened;
}

function problemKey(problem: DiagnosticProblem, index: number): string {
    return [
        problem.language,
        problem.path,
        problem.range.start.line,
        problem.range.start.character,
        problem.severity,
        problem.code,
        problem.message,
        index,
    ].join(":");
}

export function EditorInsights({ project, path, controller, visible, onNavigate, paneId }: EditorInsightsProps) {
    const [expanded, setExpanded] = useState(false);
    const [problemMenu, setProblemMenu] = useState<ProblemMenu | null>(null);
    const [tab, setTab] = useState<EditorInsightsTab>("problems");
    const [outline, setOutline] = useState<OutlineState>(EMPTY_OUTLINE);
    const [outlineRequest, setOutlineRequest] = useState(0);

    const subscribeRevision = useMemo(() => (controller ? (listener: () => void) => controller.subscribe(listener) : NOOP_SUBSCRIBE), [controller]);
    const readRevision = useMemo(() => (controller ? () => controller.getSnapshot().revision : ZERO_REVISION), [controller]);
    const revision = useSyncExternalStore(subscribeRevision, readRevision, ZERO_REVISION);
    const snapshot = controller?.getSnapshot();
    const problems = useMemo(() => {
        void revision;
        return controller ? Array.from(controller.selectProblems()) : [];
    }, [controller, revision]);
    const language = path ? languageFromPath(path) : null;
    const outlineKey = path && language ? `${project}\0${language}\0${path}` : "";
    const outlineCurrent = outline.key === outlineKey;
    const outlineStatus = outlineCurrent ? outline.status : "loading";
    const flatSymbols = useMemo(
        () => (outline.key === outlineKey ? flattenSymbols(outline.symbols) : []),
        [outline.key, outline.symbols, outlineKey],
    );

    useEffect(() => {
        if (!expanded || tab !== "outline" || !visible || !path || !language || !outlineKey) return;
        let cancelled = false;
        setOutline((current) => ({ key: outlineKey, status: "loading", symbols: current.key === outlineKey ? current.symbols : Object.freeze([]) }));
        void lsp.documentSymbols(project, language, path).then(
            (symbols) => {
                if (!cancelled) setOutline({ key: outlineKey, status: "ready", symbols });
            },
            () => {
                if (!cancelled) setOutline({ key: outlineKey, status: "error", symbols: Object.freeze([]) });
            },
        );
        return () => {
            cancelled = true;
        };
    }, [expanded, language, outlineKey, outlineRequest, path, project, tab, visible]);

    const selectTab = (next: EditorInsightsTab) => {
        setTab(next);
        setExpanded(true);
    };

    return (
        <section className={`editor-insights${expanded ? " expanded" : ""}`} aria-label="Editor insights">
            <header className="editor-insights-header">
                <button
                    type="button"
                    className={tab === "problems" && expanded ? "active" : ""}
                    aria-expanded={expanded && tab === "problems"}
                    onClick={() => selectTab("problems")}>
                    Problems <span>{snapshot?.problems ?? 0}</span>
                </button>
                <button
                    type="button"
                    className={tab === "outline" && expanded ? "active" : ""}
                    aria-expanded={expanded && tab === "outline"}
                    onClick={() => selectTab("outline")}>
                    Outline
                </button>
                <span className="editor-insights-spacer" />
                {expanded && tab === "outline" && path && language && (
                    <button type="button" className="editor-insights-quiet" onClick={() => setOutlineRequest((value) => value + 1)}>
                        refresh
                    </button>
                )}
                <button
                    type="button"
                    className="editor-insights-collapse"
                    aria-label={expanded ? "Collapse editor insights" : "Expand editor insights"}
                    onClick={() => setExpanded((value) => !value)}>
                    {expanded ? "⌄" : "⌃"}
                </button>
            </header>

            {expanded && (
                <div className="editor-insights-body">
                    {tab === "problems" && problems.length === 0 && <div className="editor-insights-empty">No project problems</div>}
                    {tab === "problems" && problems.length > 0 && (
                        <VirtualPanelRows
                            items={problems}
                            selectedIndex={-1}
                            focused={false}
                            estimateSize={26}
                            getKey={problemKey}
                            renderRow={(problem) => (
                                <button
                                    type="button"
                                    className="editor-insights-row problem"
                                    onClick={() => onNavigate(problem.path, problem.range.start.line, problem.range.start.character)}
                                    onContextMenu={(event) => {
                                        event.preventDefault();
                                        setProblemMenu({
                                            x: event.clientX,
                                            y: event.clientY,
                                            problem,
                                            sending: false,
                                            sessionId: paneId ? sessionOfPane(getState(), paneId) : undefined,
                                        });
                                    }}
                                    title={problem.message}>
                                    <span className={`editor-insights-severity ${problem.severity ?? "unknown"}`} aria-hidden="true" />
                                    <span className="editor-insights-message">{problem.message}</span>
                                    {problem.code && <span className="editor-insights-code">{problem.code}</span>}
                                    <span className="editor-insights-location">{problemLocation(project, problem)}</span>
                                </button>
                            )}
                        />
                    )}

                    {tab === "outline" && (!path || !language) && <div className="editor-insights-empty">Outline is unavailable for this file</div>}
                    {tab === "outline" && path && language && outlineStatus === "loading" && flatSymbols.length === 0 && (
                        <div className="editor-insights-empty">Loading outline…</div>
                    )}
                    {tab === "outline" && path && language && outlineStatus === "error" && (
                        <div className="editor-insights-empty">The language server could not provide an outline</div>
                    )}
                    {tab === "outline" && path && language && outlineStatus === "ready" && flatSymbols.length === 0 && (
                        <div className="editor-insights-empty">No symbols in {basename(path)}</div>
                    )}
                    {tab === "outline" && flatSymbols.length > 0 && (
                        <VirtualPanelRows
                            items={flatSymbols}
                            selectedIndex={-1}
                            focused={false}
                            estimateSize={26}
                            getKey={(item) => item.key}
                            renderRow={({ symbol, depth }) => (
                                <button
                                    type="button"
                                    className="editor-insights-row symbol"
                                    style={{ paddingLeft: 12 + Math.min(depth, 16) * 14 }}
                                    onClick={() => path && onNavigate(path, symbol.selectionRange.start.line, symbol.selectionRange.start.character)}
                                    title={symbol.detail ?? symbol.name}>
                                    <span className="editor-insights-kind">{SYMBOL_KINDS[symbol.kind] ?? "symbol"}</span>
                                    <span className="editor-insights-message">{symbol.name}</span>
                                    {symbol.detail && <span className="editor-insights-detail">{symbol.detail}</span>}
                                    <span className="editor-insights-location">{symbol.selectionRange.start.line + 1}</span>
                                </button>
                            )}
                        />
                    )}
                </div>
            )}
            {problemMenu && !problemMenu.sending && (
                <TreeContextMenu
                    x={problemMenu.x}
                    y={problemMenu.y}
                    onClose={() => setProblemMenu(null)}
                    items={[
                        {
                            label: "Go to Problem",
                            run: () =>
                                onNavigate(problemMenu.problem.path, problemMenu.problem.range.start.line, problemMenu.problem.range.start.character),
                        },
                        { label: "Copy Message", run: () => void copyText(problemMenu.problem.message).catch(reportError("copy")) },
                        { sep: true },
                        { label: "Send to Agent…", run: () => setProblemMenu({ ...problemMenu, sending: true }) },
                    ]}
                />
            )}
            {problemMenu?.sending && (
                <SendToAgentMenu
                    x={problemMenu.x}
                    y={problemMenu.y}
                    sessionId={problemMenu.sessionId}
                    delivery={() => problemDelivery(project, problemMenu.problem)}
                    onClose={() => setProblemMenu(null)}
                />
            )}
        </section>
    );
}
