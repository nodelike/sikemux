import { useCallback, useEffect, useMemo, useState } from 'react';
import type { ConnectionLike } from '@sikemux/native';

import type { AgentQuestion, ChatContext } from '@shared/wire.generated';

import { callHost, useHostCall } from '@/core/hostCalls';
import { problem } from '@/core/problem';

/** What the person wrote for a question and what they sent with it, which the host writes out as one text. */
export type WrittenAnswer = { readonly answer: string; readonly paths?: string[]; readonly context?: ChatContext[] };
type QuestionAnswer = WrittenAnswer | { readonly choice: number } | { readonly choices: number[] } | { readonly fine: true };

const NONE: AgentQuestion[] = [];
const NO_IDS: ReadonlySet<string> = new Set();
const NO_PICKS: ReadonlyMap<string, Pick> = new Map();

/** An answer held for an open question until every open one has its own, and what shows was picked; empty for a secret. */
export type Pick = { given: QuestionAnswer; shown: string };

export type ChatQuestions = {
  /** What the agent asked the person and waits on, oldest first. */
  open: AgentQuestion[];
  /** Questions the agent went on without, on an assumption. */
  assumed: AgentQuestion[];
  /** Answered, until the agent has read the answer. */
  answered: AgentQuestion[];
  /** The open question on show: the one the person chose, else the first still without an answer. */
  current: AgentQuestion | null;
  picks: ReadonlyMap<string, Pick>;
  /** Every open question has an answer, so they can go together. */
  ready: boolean;
  folded: boolean;
  /**
   * The question whatever the composer sends answers, while one is open and the person has not taken the chip off; never
   * a secret one, which has its own masked field.
   */
  composing: AgentQuestion | null;
  /** The person chose to write the answer. */
  writing: boolean;
  /** Picks a choice, or on a question that takes several, ticks or unticks it. */
  choose: (question: AgentQuestion, choice: number) => void;
  /** Holds a written answer for an open question, or answers an assumed one at once. `undo` puts it back where it was written when the host did not take it. */
  hold: (question: AgentQuestion, written: WrittenAnswer, undo?: () => void) => Promise<boolean>;
  sendAll: () => void;
  /** Answers one question at once, which shows answered straight away. */
  answer: (question: AgentQuestion, given: QuestionAnswer, undo?: () => void) => Promise<boolean>;
  show: (question: AgentQuestion) => void;
  fold: (fold: boolean) => void;
  write: (question: AgentQuestion) => void;
  stopComposing: () => void;
};

const without = <T>(set: ReadonlySet<T>, drop: readonly T[]): ReadonlySet<T> => {
  const next = new Set(set);
  for (const item of drop) next.delete(item);
  return next;
};

/**
 * The questions an agent put to the person, read with the host's `questions` call whenever `seen` changes. Answers are
 * held until every open question has one, then go together with `answerQuestions`; a lone question that takes one
 * answer goes on the tap. The first answer anywhere wins.
 */
export function useQuestions(
  connection: ConnectionLike | undefined,
  agentId: string,
  seen: string,
  onError: (problem: string) => void,
): ChatQuestions {
  // Answered from here, kept out of sight until the host's next list moves them on.
  const [sent, setSent] = useState(NO_IDS);
  const [picks, setPicks] = useState(NO_PICKS);
  const [shownId, setShownId] = useState<string | null>(null);
  const [foldedId, setFoldedId] = useState<string | null>(null);
  const [writingId, setWritingId] = useState<string | null>(null);
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(() => new Set());

  const [reads, setReads] = useState(0);
  const read = useHostCall(connection, 'questions', {}, `${seen}|${reads}`);
  const all = read.data ?? NONE;
  useEffect(() => {
    if (read.problem) onError(`could not read this agent's questions: ${read.problem}`);
  }, [read.problem, onError]);

  const { open, assumed, answered } = useMemo(() => {
    const own = all.filter((question) => question.agentId === agentId);
    const waiting = (state: AgentQuestion['state']) => own.filter((question) => question.state === state && !sent.has(question.id));
    return { open: waiting('open'), assumed: waiting('assumed'), answered: own.filter((question) => question.state === 'answered') };
  }, [all, agentId, sent]);
  const current = open.find((question) => question.id === shownId) ?? open.find((question) => !picks.has(question.id)) ?? open[0] ?? null;
  const ready = open.length > 0 && open.every((question) => picks.has(question.id));
  const folded = foldedId !== null && open[0]?.id === foldedId;
  const writing = [...open, ...assumed].find((question) => question.id === writingId && !question.secret) ?? null;
  const composing = writing ?? (current && !current.secret && !folded && !dismissed.has(current.id) ? current : null);

  const call = useCallback(
    (send: (connection: ConnectionLike) => Promise<null>, ids: string[], undo?: () => void) => {
      if (!connection) return false;
      setSent((current) => new Set([...current, ...ids]));
      setWritingId((current) => (current && ids.includes(current) ? null : current));
      send(connection)
        .then(() => {
          setPicks((current) => new Map([...current].filter(([id]) => !ids.includes(id))));
          setShownId((current) => (current && ids.includes(current) ? null : current));
        })
        .catch((error: unknown) => {
          setSent((current) => without(current, ids));
          undo?.();
          onError(problem(error));
        })
        .finally(() => setReads((count) => count + 1));
      return true;
    },
    [connection, onError],
  );

  const answer = useCallback(
    async (question: AgentQuestion, given: QuestionAnswer, undo?: () => void) =>
      call((connection) => callHost(connection, 'answerQuestion', { id: question.id, ...given }), [question.id], undo),
    [call],
  );

  const sendWith = useCallback(
    (held: ReadonlyMap<string, Pick>) => {
      if (!open.length || !open.every((question) => held.has(question.id))) return;
      const answers = open.map((question) => ({ id: question.id, ...held.get(question.id)!.given }));
      call(
        (connection) => callHost(connection, 'answerQuestions', { answers }),
        open.map((question) => question.id),
      );
    },
    [open, call],
  );

  /** Holds `pick` and moves on to the next question still without an answer, or sends a lone question. */
  const pick = useCallback(
    (question: AgentQuestion, given: Pick) => {
      const held = new Map(picks).set(question.id, given);
      setPicks(held);
      setWritingId(null);
      if (open.length === 1) return sendWith(held);
      const at = open.findIndex((asked) => asked.id === question.id);
      const next = [...open.slice(at + 1), ...open].find((asked) => !held.has(asked.id));
      if (next) setShownId(next.id);
    },
    [picks, open, sendWith],
  );

  const choose = useCallback(
    (question: AgentQuestion, choice: number) => {
      const label = (index: number) => question.choices[index]?.label ?? '';
      if (!question.multi) return pick(question, { given: { choice }, shown: label(choice) });
      const was = picks.get(question.id)?.given;
      const ticked = was && 'choices' in was ? was.choices : [];
      const choices = ticked.includes(choice) ? ticked.filter((held) => held !== choice) : [...ticked, choice].sort((a, b) => a - b);
      const held = new Map(picks);
      if (choices.length) held.set(question.id, { given: { choices }, shown: choices.map(label).join(', ') });
      else held.delete(question.id);
      setPicks(held);
    },
    [picks, pick],
  );

  const hold = useCallback(
    async (question: AgentQuestion, written: WrittenAnswer, undo?: () => void) => {
      if (question.state === 'assumed') return answer(question, written, undo);
      const words = written.answer || (written.paths ?? []).join(', ');
      pick(question, { given: written, shown: question.secret ? '' : words });
      return true;
    },
    [answer, pick],
  );

  const sendAll = useCallback(() => sendWith(picks), [sendWith, picks]);
  const show = useCallback((question: AgentQuestion) => setShownId(question.id), []);
  const fold = useCallback((fold: boolean) => setFoldedId(fold ? (open[0]?.id ?? null) : null), [open]);

  const write = useCallback((question: AgentQuestion) => {
    setWritingId(question.id);
    setDismissed((current) => (current.has(question.id) ? without(current, [question.id]) : current));
  }, []);
  const stopComposing = useCallback(() => {
    if (!composing) return;
    setWritingId(null);
    setDismissed((current) => new Set(current).add(composing.id));
  }, [composing]);

  return {
    open,
    assumed,
    answered,
    current,
    picks,
    ready,
    folded,
    composing,
    writing: writing !== null,
    choose,
    hold,
    sendAll,
    answer,
    show,
    fold,
    write,
    stopComposing,
  };
}
