import type { AgentQuestion, BoardCard, BoardColumn } from '@shared/wire.generated';

export const COLUMN_LABELS: Record<BoardColumn, string> = {
  open: 'Open',
  progress: 'In progress',
  needs: 'Needs you',
  done: 'Done',
};

/** The question the card's owner waits on the person to answer, while it works the card. */
export const cardQuestion = (card: BoardCard, questions: readonly AgentQuestion[]): AgentQuestion | undefined =>
  card.owner && card.status === 'progress'
    ? questions.find((question) => question.agentId === card.owner && question.state === 'open')
    : undefined;

/** The first words of a title, for a chip that points at the card. */
export const shortTitle = (title: string): string => {
  const words = title.split(/\s+/);
  return words.length > 3 ? `${words.slice(0, 3).join(' ')}…` : title;
};
