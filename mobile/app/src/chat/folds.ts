import { createContext, useContext, useSyncExternalStore } from 'react';

import { teamRows, type RunOpen, type TeamRow } from '@shared/chat/team';
import type { ChatMessage } from '@shared/wire.generated';

/**
 * Which tool groups and rows in a transcript the person opened or closed, kept outside the rows
 * so a row the list recycles comes back as it was left.
 */
export class Folds {
  /** Messages the person watched stream in, which are never folded short. */
  readonly streamed = new Set<string>();
  private open = new Map<string, boolean>();
  private listeners = new Set<() => void>();

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  /** Whether a group or row is open; one nobody touched follows `otherwise`. */
  isOpen(id: string, otherwise: boolean): boolean {
    return this.open.get(id) ?? otherwise;
  }

  /** Which folds under `prefix` are open, as one value that changes only when one of them does. */
  openUnder(prefix: string): string {
    return [...this.open].flatMap(([id, open]) => (open && id.startsWith(prefix) ? [id] : [])).join('\n');
  }

  set(id: string, open: boolean) {
    this.open.set(id, open);
    this.listeners.forEach((listener) => listener());
  }
}

/** Where the transcript keeps which tool groups and rows are open. */
export const FoldsContext = createContext(new Folds());

/** Whether a fold is open, as the person left it or else as `otherwise` says. */
export function useFold(id: string, otherwise: boolean): [boolean, (open: boolean) => void] {
  const folds = useContext(FoldsContext);
  const open = useSyncExternalStore(folds.subscribe, () => folds.isOpen(id, otherwise));
  return [open, (next) => folds.set(id, next)];
}

/** How a row of a finished turn shows: under the summary it opens with, hidden while its work is folded, or only its reply. */
type WorkFold = NonNullable<ChatMessage['fold']>;

type RowFold = {
  /** The fold whose summary goes above this row, its first. */
  summary: WorkFold | null;
  hidden: boolean;
  /** Which parts stay in view, or null when the row shows whole. */
  shown: readonly number[] | null;
};

const UNFOLDED: RowFold = { summary: null, hidden: false, shown: null };

/** `open` says whether the person opened the fold's work, by the fold's id; `run` is how the row shows in a run of messages between agents, when it is in one. */
export function rowFold(row: ChatMessage, open: (id: string) => boolean, run?: TeamRow): RowFold {
  if (run) return { summary: null, hidden: run.hidden, shown: run.shown };
  const fold = row.fold;
  if (!fold) return UNFOLDED;
  const summary = row.id === fold.id ? fold : null;
  // A host older than v0.6.1-nightly.5 sends no `shown`, so its rows draw whole.
  if (open(fold.id) || !fold.shown) return summary ? { summary, hidden: false, shown: null } : UNFOLDED;
  return { summary, hidden: fold.shown.length === 0, shown: fold.shown };
}

/**
 * The rows a transcript draws. A folded row that draws nothing stays out of the list: each one the list
 * has to measure moves the scroll, and a long folded turn moved it until React gave up.
 */
export function shownRows(rows: ChatMessage[], open: (id: string) => boolean, runs: ReadonlyMap<string, TeamRow>): ChatMessage[] {
  return rows.filter((row) => {
    const run = runs.get(row.id);
    const { summary, hidden } = rowFold(row, open, run);
    return !hidden || summary !== null || !!run?.head;
  });
}

/** How far the person opened each run, kept as the folds `team/<run>` and `team-all/<run>`. */
export function runsOf(rows: readonly ChatMessage[], open: (key: string) => boolean): Map<string, TeamRow> {
  return teamRows(rows, (id): RunOpen => (open(`team-all/${id}`) ? 'all' : open(`team/${id}`) ? 'open' : 'closed'));
}
