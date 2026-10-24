import { installIpcTransportForTests, MemoryIpcTransport } from "../api/transport";

type Answers = Record<string, (params: Record<string, unknown>) => unknown>;

/**
 * For tests: a stand-in core that answers a plugin's views from `answers`, by method, and can say they changed.
 * An answer that throws is the failure the core would send.
 */
export function fakeViews(answers: Answers) {
    const transport = new MemoryIpcTransport();
    const asked: { method: string; params: Record<string, unknown> }[] = [];
    transport.register("core_call", async (args) => {
        const call = args as { method?: string; params?: { method: string; params: Record<string, unknown> } } | undefined;
        if (call?.method !== "pluginView" || !call.params) return null;
        const { method, params } = call.params;
        asked.push({ method, params });
        const answer = answers[method];
        if (!answer) return { failed: { category: "invalid", message: `no view ${method}` } };
        try {
            return { value: await answer(params) };
        } catch (failed) {
            return { failed };
        }
    });
    installIpcTransportForTests(transport);
    return {
        asked,
        moved: (paths: string[] | null = null) => transport.emit("core_changed", { topic: "pluginView", project: null, paths }),
    };
}
