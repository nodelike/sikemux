import { useEffect } from "react";
import { invokeCommand as invoke } from "../api/invoke";
import { getIpcTransport } from "../api/transport";
import * as cmd from "../state/commands";
import { swallow } from "../state/toast";

export const FOCUS_AGENT_EVENT = "focus_agent";

/** Shows the agent the notch or a notification asks for, including one asked for while the page was still loading. */
export function RemoteChatBridge() {
    useEffect(() => {
        const controller = new AbortController();
        const claim = () =>
            invoke<string | null>("focus_claim").then((agentId) => {
                if (agentId !== null && !controller.signal.aborted) cmd.revealAgent(agentId);
            }, swallow("show the agent asked for"));
        getIpcTransport()
            .subscribe(FOCUS_AGENT_EVENT, () => void claim(), { signal: controller.signal })
            .then(claim)
            .catch((error: unknown) => {
                if (!controller.signal.aborted) swallow("remote chat listener")(error);
            });
        return () => controller.abort();
    }, []);
    return null;
}
