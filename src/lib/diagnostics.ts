import { invokeCommand as invoke } from "../api/invoke";
import { getIpcTransport } from "../api/transport";
import { save } from "@tauri-apps/plugin-dialog";
import { fsapi } from "../api/fs";
import { busStats } from "../state/bus";
import { resourceStats } from "../state/resources";
import { getState } from "../state/store";
import type { LayoutNode } from "../state/types";
import { workbenchRuntime } from "../workbench/runtime";
import { uiActivity, UI_ACTIVITY_LIMITS, type UiActivityRejection } from "./activity";
import { installInteractionTiming, startEventLoopMonitor, startNativeUiHeartbeat } from "./instrumentation";
import { performanceTelemetry } from "./performance";
import { shaderFieldDiagnostics } from "./shaderField";

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
            resetPerformance: () => void;
        };
    }
}

const longTasks: LongTaskEntry[] = [];
const MAX_LONG_TASKS = 200;
const runtimeErrors: { at: string; kind: "error" | "unhandledrejection"; message: string }[] = [];
const MAX_RUNTIME_ERRORS = 64;
export const MAX_RUNTIME_ERROR_MESSAGE_CHARACTERS = 512;
export const NATIVE_UI_HEARTBEAT_COMMAND = "observability_ui_heartbeat";

export function sendNativeUiHeartbeat(visible: boolean, heartbeat: number): Promise<void> {
    if (typeof visible !== "boolean") return Promise.reject(new TypeError("native heartbeat visibility must be boolean"));
    if (!Number.isInteger(heartbeat) || heartbeat < 1 || heartbeat > 0xffff_ffff) {
        return Promise.reject(new RangeError("native heartbeat sequence must be a positive u32"));
    }
    return getIpcTransport().invoke<void>(NATIVE_UI_HEARTBEAT_COMMAND, { visible, heartbeat });
}

/**
 * Diagnostics must remain safe even when an opaque rejection has hostile
 * getters/toString behavior or carries megabytes of terminal/request data.
 */
export function sanitizeRuntimeErrorMessage(value: unknown): string {
    let raw = "Unhandled runtime error";
    if (typeof value === "string") {
        raw = value;
    } else if ((typeof value === "object" || typeof value === "function") && value !== null) {
        try {
            const descriptor = Object.getOwnPropertyDescriptor(value, "message");
            if (descriptor && "value" in descriptor && typeof descriptor.value === "string") raw = descriptor.value;
        } catch {
            // Opaque runtime failures are never enumerated or stringified.
        }
    }

    const limit = MAX_RUNTIME_ERROR_MESSAGE_CHARACTERS;
    const scanLimit = Math.min(raw.length, limit * 8);
    let result = "";
    let pendingSpace = false;
    for (let index = 0; index < scanLimit && result.length < limit;) {
        const code = raw.codePointAt(index) ?? 0;
        const character = String.fromCodePoint(code);
        index += character.length;
        if (code === 27) {
            const introducer = raw.charCodeAt(index);
            if (introducer === 91) {
                index += 1;
                while (index < scanLimit) {
                    const terminal = raw.charCodeAt(index);
                    index += 1;
                    if (terminal >= 64 && terminal <= 126) break;
                }
            } else if (introducer === 93) {
                index += 1;
                while (index < scanLimit) {
                    const terminal = raw.charCodeAt(index);
                    index += 1;
                    if (terminal === 7) break;
                    if (terminal === 27 && raw.charCodeAt(index) === 92) {
                        index += 1;
                        break;
                    }
                }
            } else if (index < scanLimit) {
                index += String.fromCodePoint(raw.codePointAt(index) ?? 0).length;
            }
            pendingSpace = result.length > 0;
            continue;
        }
        if (code <= 31 || (code >= 127 && code <= 159) || /\s/u.test(character)) {
            pendingSpace = result.length > 0;
            continue;
        }
        if (pendingSpace && result.length < limit) result += " ";
        pendingSpace = false;
        if (result.length + character.length > limit) break;
        result += character;
    }
    return result.trim() || "Unhandled runtime error";
}

function recordRuntimeError(kind: "error" | "unhandledrejection", value: unknown): void {
    const message = sanitizeRuntimeErrorMessage(value);
    runtimeErrors.push({ at: new Date().toISOString(), kind, message });
    if (runtimeErrors.length > MAX_RUNTIME_ERRORS) runtimeErrors.splice(0, runtimeErrors.length - MAX_RUNTIME_ERRORS);
    countRuntimeError(kind, message, value);
}

// A promise that rejects with nobody listening stays on a list WebKit walks
// every time a later promise is settled, so a burst of them stalls the window
// long after the burst. Counting them by message says which call is the source.
const runtimeErrorCounts = new Map<string, { count: number; kind: string; firstStack: string }>();
const MAX_RUNTIME_ERROR_MESSAGES = 64;
let runtimeErrorTotal = 0;
let runtimeErrorReportAt = 0;
let runtimeErrorReportPending = false;

function countRuntimeError(kind: string, message: string, value: unknown): void {
    runtimeErrorTotal += 1;
    const seen = runtimeErrorCounts.get(message);
    if (seen) seen.count += 1;
    else if (runtimeErrorCounts.size < MAX_RUNTIME_ERROR_MESSAGES) {
        const stack = value instanceof Error && typeof value.stack === "string" ? value.stack.slice(0, 1500) : "";
        runtimeErrorCounts.set(message, { count: 1, kind, firstStack: stack });
    }
    // The counts ride along with every activity report; only the file written
    // beside them is a development convenience.
    if (!import.meta.env.DEV) return;
    if (runtimeErrorReportPending) return;
    const now = Date.now();
    const wait = Math.max(0, 2000 - (now - runtimeErrorReportAt));
    runtimeErrorReportPending = true;
    window.setTimeout(() => {
        runtimeErrorReportPending = false;
        runtimeErrorReportAt = Date.now();
        void reportRuntimeErrorCounts();
    }, wait);
}

async function reportRuntimeErrorCounts(): Promise<void> {
    const report = {
        at: new Date().toISOString(),
        total: runtimeErrorTotal,
        byMessage: [...runtimeErrorCounts.entries()]
            .map(([message, seen]) => ({ message, ...seen }))
            .sort((a, b) => b.count - a.count)
            .slice(0, 24),
    };
    try {
        await fsapi.writeFile(RUNTIME_ERROR_REPORT_PATH, JSON.stringify(report, null, 2));
    } catch {
        // The report is a debugging aid; losing it must never add to the noise.
    }
}

const RUNTIME_ERROR_REPORT_PATH = "/tmp/sikemux-runtime-errors.json";

/** The loudest rejection messages seen so far, worst first. */
export function topRuntimeErrorCounts(limit = UI_ACTIVITY_LIMITS.maxEntries): UiActivityRejection[] {
    const entries: UiActivityRejection[] = [];
    for (const [message, seen] of runtimeErrorCounts) entries.push({ message, count: seen.count });
    entries.sort((left, right) => right.count - left.count);
    return entries.length > limit ? entries.slice(0, limit) : entries;
}

function paneKindAt(node: LayoutNode, paneId: string): string | null {
    if (node.type === "pane") return node.id === paneId ? node.kind : null;
    for (const child of node.children) {
        const found = paneKindAt(child, paneId);
        if (found) return found;
    }
    return null;
}

export function focusedPaneKind(): string | null {
    const state = getState();
    const session = state.sessions[state.activeSessionId];
    const window = session ? state.windows[session.activeWindowId] : undefined;
    return window ? paneKindAt(window.root, window.activePaneId) : null;
}

export const UI_ACTIVITY_COMMAND = "observability_ui_activity";

let uiActivitySendInFlight = false;

/** Ship one activity report, unless the window is hidden or one is still going. */
export function sendUiActivity(): Promise<void> {
    if (uiActivitySendInFlight) return Promise.resolve();
    const report = uiActivity.report();
    if (!report) return Promise.resolve();
    uiActivitySendInFlight = true;
    return getIpcTransport()
        .invoke<void>(UI_ACTIVITY_COMMAND, { activity: report })
        .finally(() => {
            uiActivitySendInFlight = false;
        });
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
        visibleTerminals: document.querySelectorAll(".window-layer.live .xterm").length,
        hiddenTerminals: document.querySelectorAll(".window-layer:not(.live) .xterm").length,
        codeMirrorEditors: document.querySelectorAll(".cm-editor").length,
        visibleWindowLayers: document.querySelectorAll(".window-layer.live").length,
    };
}

function storeSnapshot() {
    const s = getState();
    return {
        sessions: Object.keys(s.sessions).length,
        windows: Object.keys(s.windows).length,
        agents: Object.keys(s.agents).length,
        agentLifecycle: {
            live: Object.values(s.agents).filter((agent) => agent.launchState !== "dormant").length,
            sleeping: Object.values(s.agents).filter((agent) => agent.launchState === "dormant").length,
            keptAlive: Object.values(s.agents).filter((agent) => agent.keepAlive === true).length,
            resumable: Object.values(s.agents).filter((agent) => !!agent.resumeId).length,
        },
        agentActivity: Object.fromEntries(
            ["unknown", "working", "blocked", "done", "idle"].map((state) => [
                state,
                Object.values(s.agentActivity).filter((activity) => activity.state === state).length,
            ]),
        ),
        editorViews: Object.keys(s.editorViews).length,
        gitViews: Object.keys(s.gitViews).length,
        gitCmdLog: s.gitCmdLog.length,
    };
}

export const REACT_PROFILER_FLAG = "sikemux.react-profiler";

/**
 * Whether to wrap the tree in React's `<Profiler>`.
 *
 * It is not free: React times every commit of every component below it and
 * calls back on each one, in a shell that commits on keystrokes. That is worth
 * paying for while working on render cost and not the rest of the time, so a
 * build only does it when someone has asked, then reloaded:
 *
 *     localStorage.setItem("sikemux.react-profiler", "on")
 */
export function reactProfilingEnabled(): boolean {
    try {
        return localStorage.getItem(REACT_PROFILER_FLAG) === "on";
    } catch {
        return false;
    }
}

export function browserDiagnostics(): Record<string, unknown> {
    return {
        at: new Date().toISOString(),
        memory: memorySnapshot(),
        dom: domSnapshot(),
        store: storeSnapshot(),
        workbench: workbenchRuntime.getSnapshot(),
        resources: resourceStats(),
        bus: busStats(),
        shaderFields: shaderFieldDiagnostics(),
        longTaskCount: longTasks.length,
        lastLongTasks: longTasks.slice(-10),
        runtimeErrors: runtimeErrors.slice(),
        uiActivity: uiActivity.snapshot(),
        performance: performanceTelemetry.snapshot(),
        performanceSemantics: {
            inputLatency: "input to next animation-frame callback; compositor presentation is not observable in WKWebView",
            eventLoopHangThresholdMs: 100,
        },
    };
}

export function nativeDiagnostics(): Promise<unknown> {
    return invoke("runtime_diagnostics");
}

export async function exportDiagnosticsSnapshot(snapshot: unknown): Promise<string | null> {
    const stamp = new Date().toISOString().replaceAll(":", "-");
    const path = await save({
        title: "Save Sikemux diagnostics",
        defaultPath: `sikemux-diagnostics-${stamp}.json`,
        filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!path) return null;
    await fsapi.writeFile(path, `${JSON.stringify(snapshot, null, 2)}\n`);
    return path;
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
        resetPerformance: () => performanceTelemetry.reset(),
    };

    uiActivity.setSources({ focusPane: focusedPaneKind, rejections: topRuntimeErrorCounts });

    installInteractionTiming();
    startEventLoopMonitor();
    startNativeUiHeartbeat({
        // Every second pulse, so activity lands about once a second without a
        // second timer of its own.
        send: (visible, heartbeat) => {
            const pulse = sendNativeUiHeartbeat(visible, heartbeat);
            if (heartbeat % 2 === 0) {
                void sendUiActivity().catch(() => performanceTelemetry.incrementCounter("ui-activity.send_errors"));
            }
            return pulse;
        },
        onError: () => performanceTelemetry.incrementCounter("watchdog.heartbeat.send_errors"),
    });

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
