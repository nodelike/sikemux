import type { MessageKind, MessageOutcome, MessageRow } from '@shared/wire.generated';

const SENDERS: Readonly<Record<string, string>> = { person: 'You', board: 'Agent board', auto: 'Auto', sikemux: 'Sikemux' };

const OUTCOMES: Readonly<Record<MessageOutcome, string>> = {
  steered: 'Read mid-turn',
  newTurn: 'Started a turn',
  typed: 'Typed in',
  waiting: 'Waiting',
  delivered: 'Delivered',
  returned: 'Not taken, waiting',
  repeat: 'Repeat, not sent',
  refused: 'Refused',
  dropped: 'Dropped unread',
};

const KINDS: Readonly<Record<MessageKind, string>> = {
  message: 'Message',
  report: 'Report',
  question: 'Question',
  answer: 'Answer',
  notice: 'Note',
};

export function outcomeLabel(row: Pick<MessageRow, 'outcome' | 'waited'>): string {
  return row.outcome === 'delivered' && row.waited ? 'Delivered after waiting' : OUTCOMES[row.outcome];
}

export function kindLabel(kind: MessageKind | null): string {
  return kind ? KINDS[kind] : 'Message';
}

/** Who sent or got a message: a sender that is not an agent by what it is, an agent by its name. */
export function whoLabel(id: string | null, name: string | null): string {
  return id ? (SENDERS[id] ?? name ?? 'A closed agent') : 'An agent';
}

/** Each row's key among `rows`, from when it was sent, who sent it and to whom. */
export function rowKeys(rows: readonly MessageRow[]): string[] {
  const seen = new Map<string, number>();
  return rows.map((row) => {
    const key = `${row.sentAt}:${row.from ?? ''}:${row.to}`;
    const repeats = seen.get(key) ?? 0;
    seen.set(key, repeats + 1);
    return repeats ? `${key}:${repeats}` : key;
  });
}
