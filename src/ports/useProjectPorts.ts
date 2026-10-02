import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { portsApi } from "../api/ports";
import { usePageVisible } from "../hooks/usePageVisible";
import { projectControllerBridge } from "../projects/controllerBridge";
import { getState, useStore } from "../state/store";
import { swallow } from "../state/toast";
import { onTaskProcessChange } from "../tasks/processSignal";
import { hasLiveWork, projectPorts, type ProjectPort } from "./projectPorts";

export const PORT_REFRESH_MS = 5_000;
/** A server binds its port a moment after its task starts. */
export const PORT_SETTLE_MS = 1_500;

const NO_PORTS: ProjectPort[] = [];

export function useProjectPreviewUrl(cwd: string | null): string | undefined {
    const snapshot = useSyncExternalStore(
        projectControllerBridge.subscribe,
        projectControllerBridge.getActiveSnapshot,
        projectControllerBridge.getActiveSnapshot,
    );
    const config = snapshot?.cwd === cwd ? snapshot.config : null;
    return config?.status === "valid" ? config.config.preview?.url : undefined;
}

/**
 * The ports the project's terminals, tasks and agents listen on. It reads them
 * every few seconds and whenever a task starts or stops, but only while the
 * window is on screen and the project has something running.
 */
export function useProjectPorts(sessionId: string | null): ProjectPort[] {
    const live = useStore((state) => (sessionId ? hasLiveWork(state, sessionId) : false));
    const cwd = useStore((state) => (sessionId ? (state.sessions[sessionId]?.cwd ?? null) : null));
    const previewUrl = useProjectPreviewUrl(cwd);
    const visible = usePageVisible();
    const [found, setFound] = useState<{ sessionId: string; ports: ProjectPort[] } | null>(null);
    const previewRef = useRef(previewUrl);
    previewRef.current = previewUrl;

    useEffect(() => {
        if (!sessionId || !live) {
            setFound(null);
            return;
        }
        if (!visible) return;
        let cancelled = false;
        let reading = false;
        const read = () => {
            if (reading) return;
            reading = true;
            portsApi
                .listening()
                .then((ports) => {
                    if (!cancelled) setFound({ sessionId, ports: projectPorts(getState(), sessionId, ports, previewRef.current) });
                })
                .catch(swallow("listening ports"))
                .finally(() => {
                    reading = false;
                });
        };
        const settles = new Set<ReturnType<typeof setTimeout>>();
        const changed = () => {
            read();
            const settle = setTimeout(() => {
                settles.delete(settle);
                read();
            }, PORT_SETTLE_MS);
            settles.add(settle);
        };
        read();
        const timer = setInterval(read, PORT_REFRESH_MS);
        const unsubscribe = onTaskProcessChange(changed);
        return () => {
            cancelled = true;
            clearInterval(timer);
            for (const settle of settles) clearTimeout(settle);
            unsubscribe();
        };
    }, [sessionId, live, visible, previewUrl]);

    return found && found.sessionId === sessionId ? found.ports : NO_PORTS;
}
