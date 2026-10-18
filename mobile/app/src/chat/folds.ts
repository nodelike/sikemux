import { createContext, useContext, useSyncExternalStore } from 'react';

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
