import type { BoardColumn } from '@shared/wire.generated';

export const COLUMN_LABELS: Record<BoardColumn, string> = {
  open: 'Open',
  progress: 'In progress',
  needs: 'Needs you',
  done: 'Done',
};

/** The first words of a title, for a chip that points at the card. */
export const shortTitle = (title: string): string => {
  const words = title.split(/\s+/);
  return words.length > 3 ? `${words.slice(0, 3).join(' ')}…` : title;
};
