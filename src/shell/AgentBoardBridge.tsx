import { useEffect } from "react";
import type { ProjectBoard } from "../agents/agentBoard";
import { AGENT_BOARD_EVENT, agentBoardApi } from "../api/agentBoard";
import { getIpcTransport } from "../api/transport";
import * as cmd from "../state/commands";
import { swallow } from "../state/toast";

/** Keeps the app's copy of each project's board, which the core sends whenever one changes. */
export function AgentBoardBridge() {
    useEffect(() => {
        const controller = new AbortController();
        void getIpcTransport()
            .subscribe<ProjectBoard[]>(AGENT_BOARD_EVENT, (event) => cmd.setAgentBoards(event.payload ?? []), { signal: controller.signal })
            .then(() => agentBoardApi.all())
            .then((boards) => {
                if (!controller.signal.aborted) cmd.setAgentBoards(boards);
            })
            .catch(swallow("read the boards"));
        return () => controller.abort();
    }, []);
    return null;
}
