import { useEffect } from "react";
import type { AcpChat } from "../api/acp";
import { getIpcTransport } from "../api/transport";
import * as cmd from "../state/commands";
import { getState } from "../state/store";
import { swallow } from "../state/toast";

export const REMOTE_CHAT_BEGUN_EVENT = "remote_chat_begun";
export const REMOTE_CHAT_WAKE_EVENT = "remote_chat_wake";

function wake(agentId: string) {
    if (getState().agents[agentId]?.launchState === "dormant") cmd.resumeAgent(agentId);
}

/** Shows a chat a paired phone starts among its project's agents, and wakes a sleeping one a phone opens. */
export function RemoteChatBridge() {
    useEffect(() => {
        const controller = new AbortController();
        const transport = getIpcTransport();
        Promise.all([
            transport.subscribe<AcpChat>(REMOTE_CHAT_BEGUN_EVENT, (event) => void cmd.adoptChat(event.payload), { signal: controller.signal }),
            transport.subscribe<string>(REMOTE_CHAT_WAKE_EVENT, (event) => wake(event.payload), { signal: controller.signal }),
        ]).catch((error: unknown) => {
            if (!controller.signal.aborted) swallow("remote chat listener")(error);
        });
        return () => controller.abort();
    }, []);
    return null;
}
