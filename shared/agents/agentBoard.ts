import type { AgentQuestion } from "../wire.generated";
import type { BoardCard, BoardColumn, BoardSettings, CardStatus, CardTicket, ProjectBoard, RunStage } from "../wire.generated";

export type {
    BoardCard,
    BoardColumn,
    BoardSettings,
    CardRun,
    CardStatus,
    CardTicket,
    ProjectBoard,
    ReviewerChoice,
    RunStage,
} from "../wire.generated";

export type SettingsChanges = Partial<BoardSettings>;

/** What the core says of a project's Auto beside its cards. */
export type BoardAuto = Pick<ProjectBoard, "settings" | "running" | "autoNote">;

export const RUN_WAITS_ON_PERSON: ReadonlySet<RunStage> = new Set(["review", "held"]);

export const COLUMN_LABELS: Record<BoardColumn, string> = {
    open: "Open",
    progress: "In progress",
    needs: "Needs you",
    done: "Done",
};

/** What a board reads of a question; the phone's own copy of the type fits it too. */
type CardAsk = Pick<AgentQuestion, "agentId" | "state">;

/** The question the card's owner waits on the person to answer, while it works the card. */
export const cardQuestion = <Q extends CardAsk>(card: BoardCard, questions: readonly Q[]): Q | undefined =>
    card.owner && card.status === "progress" ? questions.find((question) => question.agentId === card.owner && question.state === "open") : undefined;

/** The first words of a title, for a chip that points at the card. */
export const shortTitle = (title: string): string => {
    const words = title.split(/\s+/);
    return words.length > 3 ? `${words.slice(0, 3).join(" ")}…` : title;
};

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
