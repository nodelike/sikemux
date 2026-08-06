import { invoke } from "@tauri-apps/api/core";
import { busStats } from "../state/bus";
import { resourceStats } from "../state/resources";
import { getState } from "../state/store";

type LongTaskEntry = {
    name: string;
    startTime: number;
    duration: number;
};

type MemoryInfo = {
    usedJSHeapSize?: number;
    totalJSHeapSize?: number;
    jsHeapSizeLimit?: number;
};

declare global {
    interface Window {
        sikemuxDiagnostics?: {
            snapshot: () => Record<string, unknown>;
            native: () => Promise<unknown>;
            longTasks: () => LongTaskEntry[];
            clearLongTasks: () => void;
        };
    }
}

const longTasks: LongTaskEntry[] = [];
const MAX_LONG_TASKS = 200;
const runtimeErrors: { at: string; kind: "error" | "unhandledrejection"; message: string }[] = [];
const MAX_RUNTIME_ERRORS = 64;

function recordRuntimeError(kind: "error" | "unhandledrejection", value: unknown): void {
    const message = value instanceof Error ? value.message : typeof value === "string" ? value : String(value);
    runtimeErrors.push({ at: new Date().toISOString(), kind, message });
    if (runtimeErrors.length > MAX_RUNTIME_ERRORS) runtimeErrors.splice(0, runtimeErrors.length - MAX_RUNTIME_ERRORS);
}

function memorySnapshot(): MemoryInfo | null {
    const perf = performance as Performance & { memory?: MemoryInfo };
    return perf.memory ?? null;
}

function domSnapshot() {
    return {
        elements: document.getElementsByTagName("*").length,
        xterms: document.querySelectorAll(".xterm").length,
        canvases: document.querySelectorAll("canvas").length,
        terminalRenderers: {
            dom: document.querySelectorAll('[data-terminal-renderer="dom"]').length,
            webgl: document.querySelectorAll('[data-terminal-renderer="webgl"]').length,
        },
        visibleTerminals: document.querySelectorAll(".window-layer.visible .xterm").length,
        hiddenTerminals: document.querySelectorAll(".window-layer:not(.visible) .xterm").length,
        codeMirrorEditors: document.querySelectorAll(".cm-editor").length,
        visibleWindowLayers: document.querySelectorAll(".window-layer.visible").length,
    };
}

function storeSnapshot() {
    const s = getState();
    return {
        sessions: Object.keys(s.sessions).length,
        windows: Object.keys(s.windows).length,
        agents: Object.keys(s.agents).length,
        agentActivity: Object.fromEntries(
            ["unknown", "working", "blocked", "done", "idle"].map((state) => [
                state,
                Object.values(s.agentActivity).filter((activity) => activity.state === state).length,
            ]),
        ),
        editorViews: Object.keys(s.editorViews).length,
        gitViews: Object.keys(s.gitViews).length,
        rundeckViews: Object.keys(s.rundeckViews).length,
        brunoViews: Object.keys(s.brunoViews).length,
        gitCmdLog: s.gitCmdLog.length,
    };
}

export function browserDiagnostics(): Record<string, unknown> {
    return {
        at: new Date().toISOString(),
        memory: memorySnapshot(),
        dom: domSnapshot(),
        store: storeSnapshot(),
        resources: resourceStats(),
        bus: busStats(),
        longTaskCount: longTasks.length,
        lastLongTasks: longTasks.slice(-10),
        runtimeErrors: runtimeErrors.slice(),
    };
}

export function nativeDiagnostics(): Promise<unknown> {
    return invoke("runtime_diagnostics");
}

export function installDiagnostics(): void {
    if (window.sikemuxDiagnostics) return;

    window.sikemuxDiagnostics = {
        snapshot: browserDiagnostics,
        native: nativeDiagnostics,
        longTasks: () => longTasks.slice(),
        clearLongTasks: () => {
            longTasks.length = 0;
        },
    };

    window.addEventListener("error", (event) => recordRuntimeError("error", event.error ?? event.message));
    window.addEventListener("unhandledrejection", (event) => recordRuntimeError("unhandledrejection", event.reason));

    if (typeof PerformanceObserver !== "undefined") {
        try {
            const observer = new PerformanceObserver((list) => {
                for (const entry of list.getEntries()) {
                    longTasks.push({
                        name: entry.name,
                        startTime: entry.startTime,
                        duration: entry.duration,
                    });
                    if (longTasks.length > MAX_LONG_TASKS) longTasks.splice(0, longTasks.length - MAX_LONG_TASKS);
                }
            });
            observer.observe({ type: "longtask", buffered: true });
        } catch {
            // WebKit may not expose longtask; snapshot diagnostics still work.
        }
    }
}
