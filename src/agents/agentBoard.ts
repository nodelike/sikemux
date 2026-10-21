import { blockingQuestion, type AgentQuestion } from "./agentQuestions";
import type { BoardCard, BoardSettings, CardStatus, CardTicket, ProjectBoard, RunStage } from "../api/generated/wire";

export type { BoardCard, BoardSettings, CardRun, CardStatus, CardTicket, ProjectBoard, ReviewerChoice, RunStage } from "../api/generated/wire";

export type SettingsChanges = Partial<BoardSettings>;

/** What the core says of a project's Auto beside its cards. */
export type BoardAuto = Pick<ProjectBoard, "settings" | "running" | "autoNote">;

export const RUN_WAITS_ON_PERSON: ReadonlySet<RunStage> = new Set(["review", "held"]);

/** Where a board shows a card: "needs" holds the cards waiting on the person. */
export type BoardColumn = CardStatus | "needs";

export const COLUMN_LABELS: Record<BoardColumn, string> = {
    open: "Open",
    progress: "In progress",
    needs: "Needs you",
    done: "Done",
};

/** The question the card's owner waits on the person to answer, while it works the card. */
export const cardQuestion = (card: BoardCard, questions: readonly AgentQuestion[]): AgentQuestion | undefined =>
    card.owner && card.status === "progress" ? blockingQuestion(questions, card.owner) : undefined;

/** The column a card shows in, at `status` while a move to it is on its way. */
export function cardColumn(card: BoardCard, questions: readonly AgentQuestion[], status: CardStatus = card.status): BoardColumn {
    if (status === "done") return status;
    if (card.auto && RUN_WAITS_ON_PERSON.has(card.auto.stage)) return "needs";
    return status === "progress" && cardQuestion(card, questions) ? "needs" : status;
}

/** The first words of a title, for a chip that points at the card. */
export const shortTitle = (title: string): string => {
    const words = title.split(/\s+/);
    return words.length > 3 ? `${words.slice(0, 3).join(" ")}…` : title;
};

/** The ticket a card is filed under: its own, or the nearest card above it that has one. */
export function ticketAbove(card: BoardCard, byId: ReadonlyMap<number, BoardCard>): string | null {
    let current = card.parent === null ? undefined : byId.get(card.parent);
    for (let steps = 0; current && steps < byId.size; steps += 1) {
        if (current.ticket) return current.ticket.key;
        current = current.parent === null ? undefined : byId.get(current.parent);
    }
    return null;
}

/** What the person may change on a card. A parent of 0 takes the card out from under one. */
export interface CardChanges {
    readonly title?: string;
    readonly details?: string;
    readonly acceptance?: string;
    readonly status?: CardStatus;
    readonly dependsOn?: number[];
    readonly parent?: number;
}

export interface NewCard {
    readonly title: string;
    readonly details?: string;
    readonly acceptance?: string;
    readonly parent?: number;
    readonly dependsOn?: number[];
    readonly ticket?: CardTicket;
}
