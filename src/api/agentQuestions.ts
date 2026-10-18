import type { AgentQuestion, QuestionAnswer } from "../agents/agentQuestions";
import { invokeCommand as invoke } from "./invoke";

/** Sent by the window whenever the open questions change. */
export const AGENT_QUESTIONS_EVENT = "agent_questions";

export const agentQuestionsApi = {
    list: () => invoke<AgentQuestion[]>("agent_question_call", { method: "questions", params: null }),
    answer: (id: string, answer: QuestionAnswer) =>
        invoke<{ answered: boolean }>("agent_question_call", { method: "answerQuestion", params: { id, ...answer } }),
    /** Answers questions asked together, so each agent reads its answers in one message. */
    answerAll: (answers: readonly { id: string; answer: QuestionAnswer }[]) =>
        invoke<{ answered: boolean }>("agent_question_call", {
            method: "answerQuestions",
            params: { answers: answers.map(({ id, answer }) => ({ id, ...answer })) },
        }),
};
