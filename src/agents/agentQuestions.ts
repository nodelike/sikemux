export interface QuestionChoice {
    readonly label: string;
    readonly detail?: string;
}

/** A question an agent asked with `ask` or its CLI's own tool, as the core keeps it until someone answers. */
export interface AgentQuestion {
    readonly id: string;
    readonly agentId: string;
    /** The asker's name, such as Maple. */
    readonly name: string;
    readonly cli: string;
    readonly project: string;
    /** The asker's own words. */
    readonly asked: string;
    /** What the person is asked, which a lead that passed it on may have reworded. */
    readonly text: string;
    /** Two or three words naming the question, such as Photo access. */
    readonly header?: string;
    readonly choices: QuestionChoice[];
    /** The person may pick more than one of `choices`. */
    readonly multi?: boolean;
    /** The asker waits for the answer; otherwise it goes on with `assumption`. */
    readonly blocked: boolean;
    readonly assumption: string | null;
    /** The answer is a secret, such as a password: typed into a masked field, never the composer, and never shown again. */
    readonly secret: boolean;
    readonly at: number;
    /** The lead it waits on, while it has not reached the person. */
    readonly lead: string | null;
    /** The lead it went to first, once it reached the person. */
    readonly via: string | null;
    /** That lead passed it on itself rather than leaving it unanswered. */
    readonly passed: boolean;
    /** The ticket of the board card the asker is working, such as PROJ-12. */
    readonly ticket?: string;
}

export type QuestionAnswer =
    { readonly answer: string } | { readonly choice: number } | { readonly choices: readonly number[] } | { readonly fine: true };

/** Beside the field a secret is typed into. */
export const secretNote = (asker: string): string =>
    `Kept private. ${asker} reads it from a file only your account can open, and it never shows in the chat.`;

/** A secret's answer once sent, in place of its text. */
export const SECRET_ANSWERED = "Answered, kept private";

export const asksPerson = (question: AgentQuestion): boolean => question.lead === null;

/** The question the agent waits on the person to answer, the first it asked. */
export const blockingQuestion = (questions: readonly AgentQuestion[], agentId: string): AgentQuestion | undefined =>
    questions.find((question) => question.agentId === agentId && question.blocked && asksPerson(question));

/** A question the agent went on without, on an assumption the person has not answered. */
export const assumedQuestion = (questions: readonly AgentQuestion[], agentId: string): AgentQuestion | undefined =>
    questions.find((question) => question.agentId === agentId && !question.blocked && asksPerson(question));

export const waitsOnPerson = (questions: readonly AgentQuestion[], agentId: string): boolean => blockingQuestion(questions, agentId) !== undefined;

/** Why a question reached the person rather than the lead it went to first. */
export function viaNote(question: Pick<AgentQuestion, "via" | "passed">): string | null {
    if (!question.via) return null;
    return question.passed ? `${question.via} didn't know` : `${question.via} didn't answer`;
}
