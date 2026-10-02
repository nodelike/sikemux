import { useEffect } from "react";
import { invokeCommand as invoke } from "../api/invoke";
import { getIpcTransport } from "../api/transport";
import { getCurrentWindow } from "@tauri-apps/api/window";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { errMessage, swallow } from "../state/toast";
import type { CliFrontendRequest, CliOpenResult } from "../state/types";

async function acknowledge(result: CliOpenResult): Promise<void> {
    await invoke("cli_open_result", { result });
}

/** Connects the hydrated UI to the backend CLI broker. It renders no UI. */
export function CliOpenBridge() {
    useEffect(() => {
        let disposed = false;
        let claiming = false;
        let claimAgain = false;
        let readyPending = false;
        const listenerController = new AbortController();
        let retryTimer: ReturnType<typeof setTimeout> | null = null;
        let retryDelay = 100;

        const focusApp = async () => {
            const win = getCurrentWindow();
            await win.show();
            await win.unminimize();
            await win.setFocus();
        };

        const processRequests = async (requests: CliFrontendRequest[]) => {
            for (const { request } of requests) {
                if (disposed) return;
                let immediate: CliOpenResult[];
                try {
                    immediate = cmd.routeCliOpenRequest(request);
                } catch (error) {
                    immediate = request.targets.map((target) => ({
                        requestId: request.id,
                        targetId: target.id,
                        paneId: null,
                        path: target.path,
                        error: errMessage(error),
                    }));
                }
                await focusApp().catch(swallow("CLI window focus"));
                for (const result of immediate) {
                    if (disposed) return;
                    await acknowledge(result);
                }
            }
        };

        const claim = async (ready = false) => {
            if (ready) readyPending = true;
            claimAgain = true;
            if (claiming) return;
            claiming = true;
            try {
                while (claimAgain && !disposed) {
                    claimAgain = false;
                    const command = readyPending ? "cli_frontend_ready" : "cli_claim_open_requests";
                    readyPending = false;
                    const requests = await invoke<CliFrontendRequest[]>(command);
                    await processRequests(requests);
                }
                retryDelay = 100;
            } catch (error) {
                swallow("CLI open claim")(error);
                if (!disposed && retryTimer === null) {
                    retryTimer = setTimeout(() => {
                        retryTimer = null;
                        void claim(true);
                    }, retryDelay);
                    retryDelay = Math.min(retryDelay * 2, 2_000);
                }
            } finally {
                claiming = false;
            }
        };

        void (async () => {
            await getIpcTransport().subscribe("cli-open-available", () => void claim(), { signal: listenerController.signal });
            if (disposed) return;
            await claim(true);
        })().catch((error: unknown) => {
            if (!listenerController.signal.aborted) swallow("CLI open listener")(error);
        });

        const unsubscribeStore = useStore.subscribe((state, previous) => {
            for (const [paneId, previousView] of Object.entries(previous.editorViews)) {
                const currentTabs = new Set(state.editorViews[paneId]?.openTabs ?? []);
                const closed = previousView.openTabs.filter((path) => !currentTabs.has(path));
                if (closed.length > 0) {
                    void invoke("cli_editor_tabs_closed", { paneId, paths: closed }).catch(swallow("CLI tab close"));
                }
            }
        });

        return () => {
            disposed = true;
            listenerController.abort();
            if (retryTimer !== null) clearTimeout(retryTimer);
            unsubscribeStore();
        };
    }, []);

    return null;
}
