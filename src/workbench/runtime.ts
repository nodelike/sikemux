import { performanceTelemetry } from "../lib/performance";
import type { StoreState } from "../state/store";
import { useStore } from "../state/store";
import { collectPanes } from "../state/layout";
import type { Window } from "../state/types";
import { SessionController } from "./sessionController";

export interface WorkbenchRuntimeSnapshot {
    readonly sessions: number;
    readonly items: number;
    readonly reconciliations: number;
    readonly started: boolean;
}

interface TopologyRefs {
    readonly activeSessionId: string;
    readonly sessions: StoreState["sessions"];
    readonly sessionOrder: StoreState["sessionOrder"];
    readonly windows: StoreState["windows"];
    readonly windowsBySession: StoreState["windowsBySession"];
}

function topologyRefs(state: StoreState): TopologyRefs {
    return {
        activeSessionId: state.activeSessionId,
        sessions: state.sessions,
        sessionOrder: state.sessionOrder,
        windows: state.windows,
        windowsBySession: state.windowsBySession,
    };
}

const paneSignatures = new WeakMap<Window, string>();

/* The items a window holds and which one is active, without the split sizes a resize changes on every frame. */
function paneSignature(window: Window): string {
    let signature = paneSignatures.get(window);
    if (signature === undefined) {
        signature = `${window.activePaneId}|${collectPanes(window.root)
            .map((pane) => `${pane.id}/${pane.kind}`)
            .join(",")}`;
        paneSignatures.set(window, signature);
    }
    return signature;
}

function samePanes(left: StoreState["windows"], right: StoreState["windows"]): boolean {
    if (left === right) return true;
    const ids = Object.keys(right);
    if (ids.length !== Object.keys(left).length) return false;
    return ids.every((id) => {
        const before = left[id];
        const after = right[id];
        return before !== undefined && (before === after || paneSignature(before) === paneSignature(after));
    });
}

function sameTopology(left: TopologyRefs | null, right: TopologyRefs): boolean {
    return (
        left !== null &&
        left.activeSessionId === right.activeSessionId &&
        left.sessions === right.sessions &&
        left.sessionOrder === right.sessionOrder &&
        left.windowsBySession === right.windowsBySession &&
        samePanes(left.windows, right.windows)
    );
}

export class WorkbenchRuntime {
    private readonly sessions = new Map<string, SessionController>();
    private unsubscribe: (() => void) | null = null;
    private lastTopology: TopologyRefs | null = null;
    private reconciliations = 0;

    start(): void {
        if (this.unsubscribe) return;
        this.reconcileIfChanged(useStore.getState());
        this.unsubscribe = useStore.subscribe((state) => this.reconcileIfChanged(state));
    }

    private reconcileIfChanged(state: StoreState): void {
        const nextTopology = topologyRefs(state);
        if (sameTopology(this.lastTopology, nextTopology)) return;
        this.lastTopology = nextTopology;
        this.reconcile(state);
    }

    reconcile(state: StoreState): void {
        const span = performanceTelemetry.startTrace("workbench.reconcile", { sessions: state.sessionOrder.length });
        try {
            const live = new Set(state.sessionOrder);
            for (const [sessionId, controller] of this.sessions) {
                if (live.has(sessionId)) continue;
                controller.dispose();
                this.sessions.delete(sessionId);
            }
            const windowsOf = (sessionId: string) =>
                (state.windowsBySession[sessionId] ?? []).map((id) => state.windows[id]).filter((window) => window !== undefined);
            for (const [sessionId, controller] of this.sessions) controller.release(windowsOf(sessionId));
            for (const sessionId of state.sessionOrder) {
                const session = state.sessions[sessionId];
                if (!session) continue;
                let controller = this.sessions.get(sessionId);
                if (!controller) {
                    controller = new SessionController(sessionId);
                    this.sessions.set(sessionId, controller);
                }
                controller.reconcile(session, windowsOf(sessionId), state.activeSessionId);
            }
            this.reconciliations += 1;
            performanceTelemetry.incrementCounter("workbench.reconciliations");
            performanceTelemetry.setGauge("workbench.sessions", this.sessions.size);
            performanceTelemetry.setGauge("workbench.items", this.itemCount());
            const recorded = performanceTelemetry.endSpan(span, { outcome: "success" });
            if (recorded) performanceTelemetry.recordLatency("workbench.reconcile", recorded.durationMs);
        } catch (error) {
            performanceTelemetry.endSpan(span, { outcome: "error" });
            throw error;
        }
    }

    stop(): void {
        this.unsubscribe?.();
        this.unsubscribe = null;
        this.lastTopology = null;
        this.sessions.forEach((controller) => controller.dispose());
        this.sessions.clear();
        performanceTelemetry.setGauge("workbench.sessions", 0);
        performanceTelemetry.setGauge("workbench.items", 0);
    }

    getSession(sessionId: string): SessionController | undefined {
        return this.sessions.get(sessionId);
    }

    private itemCount(): number {
        let items = 0;
        this.sessions.forEach((controller) => {
            items += controller.getSnapshot().itemCount;
        });
        return items;
    }

    getSnapshot(): WorkbenchRuntimeSnapshot {
        return {
            sessions: this.sessions.size,
            items: this.itemCount(),
            reconciliations: this.reconciliations,
            started: this.unsubscribe !== null,
        };
    }
}

export const workbenchRuntime = new WorkbenchRuntime();
