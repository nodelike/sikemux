import { useEffect } from "react";
import { AGENT_QUESTIONS_EVENT, agentQuestionsApi, type AgentQuestion } from "../api/agentQuestions";
import { getIpcTransport } from "../api/transport";
import * as cmd from "../state/commands";
import { swallow } from "../state/toast";

/** Keeps the app's copy of the questions agents asked, which the core sends whenever they change. */
export function AgentQuestionBridge() {
    useEffect(() => {
        const controller = new AbortController();
        void getIpcTransport()
            .subscribe<AgentQuestion[]>(AGENT_QUESTIONS_EVENT, (event) => cmd.setAgentQuestions(event.payload ?? []), { signal: controller.signal })
            .then(() => agentQuestionsApi.list())
            .then((questions) => {
                if (!controller.signal.aborted) cmd.setAgentQuestions(questions);
            })
            .catch(swallow("read agents' questions"));
        return () => controller.abort();
    }, []);
    return null;
}
