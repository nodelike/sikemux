import { messageLetters } from "../chat/agentLetters";
import type { ChatMessage } from "../chat/types";

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
    | { readonly answer: string }
    | { readonly choice: number }
    | { readonly choices: readonly number[] }
    | { readonly fine: true }
    | { readonly skip: true };

/** Beside the field a secret is typed into. */
export const secretNote = (asker: string): string =>
    `Kept private. ${asker} reads it from a file only your account can open, and it never shows in the chat.`;

/** A secret's answer once sent, in place of its text. */
export const SECRET_ANSWERED = "Answered, kept private";

/** A question the person skipped, in place of an answer. */
export const SKIPPED = "You skipped this question";

export const asksPerson = (question: AgentQuestion): boolean => question.lead === null;

/** An answer sent from this app. `answer` is what the chat shows until the agent has it; none for an assumption let stand. */
export interface SentAnswer {
    question: AgentQuestion;
    answer: string | null;
    skipped: boolean;
}

/** The agent's questions to the person not yet answered from here: those it waits on, and those it went on without. */
export function waitingQuestions(
    all: readonly AgentQuestion[],
    agentId: string,
    sent: readonly SentAnswer[],
): { open: AgentQuestion[]; assumed: AgentQuestion[] } {
    const unanswered = all.filter(
        (question) => question.agentId === agentId && asksPerson(question) && !sent.some((answered) => answered.question.id === question.id),
    );
    return { open: unanswered.filter((question) => question.blocked), assumed: unanswered.filter((question) => !question.blocked) };
}

/** Answers sent from here that the agent's transcript does not hold yet. */
export function unreadAnswers(
    sent: readonly SentAnswer[],
    messages: readonly ChatMessage[],
): { question: AgentQuestion; answer: string; skipped: boolean }[] {
    const shown = sent.flatMap(({ question, answer, skipped }) => (answer === null ? [] : [{ question, answer, skipped }]));
    if (shown.length === 0) return shown;
    const arrived = new Set(
        messages.flatMap((message) => (messageLetters(message) ?? []).flatMap((letter) => (letter.kind === "answer" ? [letter.question] : []))),
    );
    return shown.filter(({ question }) => !arrived.has(question.asked));
}

/** The question the agent waits on the person to answer, the first it asked. */
export const blockingQuestion = (questions: readonly AgentQuestion[], agentId: string): AgentQuestion | undefined =>
    questions.find((question) => question.agentId === agentId && question.blocked && asksPerson(question));

/** Why a question reached the person rather than the lead it went to first. */
export function viaNote(question: Pick<AgentQuestion, "via" | "passed">): string | null {
    if (!question.via) return null;
    return question.passed ? `${question.via} didn't know` : `${question.via} didn't answer`;
}
