import type { AgentQuestion } from "./generated/wire";
import { coreCall } from "./core";

export type { AgentQuestion, QuestionChoice } from "./generated/wire";

export type QuestionAnswer =
    { readonly answer: string } | { readonly choice: number } | { readonly choices: readonly number[] } | { readonly fine: true };

/** Sent by the window whenever the open questions change. */
export const AGENT_QUESTIONS_EVENT = "agent_questions";

export const agentQuestionsApi = {
    list: () => coreCall<AgentQuestion[]>("questions"),
    answer: (id: string, answer: QuestionAnswer) => coreCall<{ answered: boolean }>("answerQuestion", { id, ...answer }),
    /** Answers questions asked together, so each agent reads its answers in one message. */
    answerAll: (answers: readonly { id: string; answer: QuestionAnswer }[]) =>
        coreCall<{ answered: boolean }>("answerQuestions", { answers: answers.map(({ id, answer }) => ({ id, ...answer })) }),
};
