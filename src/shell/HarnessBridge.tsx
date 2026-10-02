import { useEffect } from "react";
import { invokeCommand } from "../api/invoke";
import { getIpcTransport } from "../api/transport";
import type { HarnessRequest } from "../harness/service";
import { useStore } from "../state/store";
import { errMessage, swallow } from "../state/toast";

/** Answers the tool calls the core hands to the window, and stops the harness tasks of a project or agent that closes. */
export function HarnessBridge() {
    useEffect(() => {
        const controller = new AbortController();
        let service: Promise<typeof import("../harness/service")> | undefined;
        const getService = () =>
            (service ??= import("../harness/service").catch((error: unknown) => {
                service = undefined;
                throw error;
            }));
        let claiming = false;
        let again = false;
        const process = async (request: HarnessRequest) => {
            try {
                const result = await (await getService()).handleHarnessRequest(request, controller.signal);
                await invokeCommand("harness_reply", { id: request.id, result, error: null });
            } catch (error) {
                await invokeCommand("harness_reply", { id: request.id, result: null, error: errMessage(error) }).catch(swallow("harness reply"));
            }
        };
        const claim = async () => {
            again = true;
            if (claiming) return;
            claiming = true;
            try {
                while (again && !controller.signal.aborted) {
                    again = false;
                    const requests = await invokeCommand<HarnessRequest[]>("harness_claim");
                    for (const request of requests) void process(request);
                }
            } finally {
                claiming = false;
            }
        };
        void (async () => {
            await getIpcTransport().subscribe(
                "harness-request",
                () => {
                    void claim().catch(swallow("harness claim"));
                },
                { signal: controller.signal },
            );
            if (!controller.signal.aborted) await claim();
        })().catch(swallow("harness bridge"));
        const stopRuns = (selector: { project: string } | { agentId: string }) =>
            void invokeCommand("harness_stop_runs", selector).catch(swallow("harness stop"));
        const unsubscribe = useStore.subscribe((state, previous) => {
            for (const session of Object.values(previous.sessions)) {
                if (
                    session.kind === "project" &&
                    !Object.values(state.sessions).some((current) => current.kind === "project" && current.cwd === session.cwd)
                )
                    stopRuns({ project: session.cwd });
            }
            if (state.agents === previous.agents) return;
            for (const agentId of Object.keys(previous.agents)) if (!state.agents[agentId]) stopRuns({ agentId });
        });
        return () => {
            controller.abort();
            unsubscribe();
        };
    }, []);
    return null;
}
