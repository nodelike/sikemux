import { useEffect } from "react";
import { invokeCommand } from "../api/invoke";
import { getIpcTransport } from "../api/transport";
import type { HarnessRequest } from "../harness/service";
import { useStore } from "../state/store";
import { errMessage, swallow } from "../state/toast";

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
            await getIpcTransport().subscribe<number>(
                "harness-task-output",
                (event) => {
                    if (service) void service.then(({ harnessTasks }) => harnessTasks.output(event.payload)).catch(swallow("harness output"));
                },
                {
                    signal: controller.signal,
                },
            );
            if (!controller.signal.aborted) await claim();
        })().catch(swallow("harness bridge"));
        const unsubscribe = useStore.subscribe((state, previous) => {
            for (const session of Object.values(previous.sessions)) {
                if (
                    service &&
                    session.kind === "project" &&
                    !Object.values(state.sessions).some((current) => current.kind === "project" && current.cwd === session.cwd)
                )
                    void service.then(({ harnessTasks }) => harnessTasks.closeProject(session.cwd)).catch(swallow("harness project close"));
            }
        });
        return () => {
            controller.abort();
            unsubscribe();
        };
    }, []);
    return null;
}
