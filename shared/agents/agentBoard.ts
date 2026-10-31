import type { AgentQuestion } from "../wire.generated";
import type { BoardCard, BoardColumn } from "../wire.generated";

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
