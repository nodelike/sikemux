import { useEffect, useRef, type Dispatch, type RefObject } from "react";
import { agentApi } from "../api/agents";
import type { Agent } from "../state/types";
import { guessClaudeWindow } from "./contextWindow";
import type { ChatAction, ChatState } from "./types";

export function useSavedUsage({
    agentRef,
    agentId,
    agentResumeId,
    cwd,
    configPath,
    connection,
    reported,
    setup,
    dispatch,
}: {
    agentRef: RefObject<Agent>;
    agentId: string;
    agentResumeId?: string;
    cwd: string;
    configPath?: string;
    connection: ChatState["connection"];
    reported: boolean;
    setup: Record<string, unknown>;
    dispatch: Dispatch<ChatAction>;
}): void {
    const setupRef = useRef(setup);
    setupRef.current = setup;
    useEffect(() => {
        const { resumeId, type } = agentRef.current;
        if (connection !== "ready" || reported || !resumeId || (type !== "claude" && type !== "codex")) return;
        let current = true;
        void agentApi
            .sessionContext(type, cwd, resumeId, configPath)
            .then((saved) => {
                if (!current || !saved) return;
                const size = saved.size ?? guessClaudeWindow(setupRef.current, agentRef.current.model, saved.used);
                dispatch({ type: "saved_usage", usage: { used: saved.used, size } });
            })
            .catch(() => {});
        return () => {
            current = false;
        };
    }, [agentRef, agentId, agentResumeId, cwd, configPath, connection, reported, dispatch]);
}
