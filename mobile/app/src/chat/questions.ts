import { useCallback, useEffect, useMemo, useState } from 'react';
import type { ConnectionLike } from '@sikemux/native';

import type { QuestionAnswer } from '@shared/agents/questions';
import type { AgentQuestion } from '@/core/protocol';

import { useHostCall } from '@/core/hostCalls';
import { problem } from '@/core/problem';

const NONE: AgentQuestion[] = [];
const NO_IDS: ReadonlySet<string> = new Set();

export type ChatQuestions = {
  /** What the agent asked the person and waits on, oldest first. */
  open: AgentQuestion[];
  /** Questions the agent went on without, on an assumption. */
  assumed: AgentQuestion[];
  /** Answered, until the agent has read the answer. */
  answered: AgentQuestion[];
  /**
   * The question whatever the composer sends answers, while one is open and the person has not taken the chip off; never
   * a secret one, which has its own masked field.
   */
  composing: AgentQuestion | null;
  /** The person chose to write the answer. */
  writing: boolean;
  /** Shows the question answered at once. `undo` puts the answer back where it was written when the host did not take it. */
  answer: (question: AgentQuestion, given: QuestionAnswer, undo?: () => void) => Promise<boolean>;
  write: (question: AgentQuestion) => void;
  stopComposing: () => void;
};

/**
 * The questions an agent put to the person, read with the host's `questions` call whenever `seen` changes, and
 * answered with `answerQuestion`. The first answer anywhere wins.
 */
export function useQuestions(
  connection: ConnectionLike | undefined,
  agentId: string,
  seen: string,
  onError: (problem: string) => void,
): ChatQuestions {
  // Answered from here, kept out of sight until the host's next list moves them on.
  const [sent, setSent] = useState(NO_IDS);
  const [writingId, setWritingId] = useState<string | null>(null);
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(() => new Set());

  const [reads, setReads] = useState(0);
  const read = useHostCall<AgentQuestion[]>(connection, 'questions', null, `${seen}|${reads}`);
  const all = read.data ?? NONE;
  useEffect(() => {
    if (read.problem) onError(`could not read this agent's questions: ${read.problem}`);
  }, [read.problem, onError]);

  const { open, assumed, answered } = useMemo(() => {
    const own = all.filter((question) => question.agentId === agentId);
    const waiting = (state: AgentQuestion['state']) => own.filter((question) => question.state === state && !sent.has(question.id));
    return { open: waiting('open'), assumed: waiting('assumed'), answered: own.filter((question) => question.state === 'answered') };
  }, [all, agentId, sent]);
  const writing = [...open, ...assumed].find((question) => question.id === writingId && !question.secret) ?? null;
  const composing = writing ?? open.find((question) => !question.secret && !dismissed.has(question.id)) ?? null;

  const answer = useCallback(
    async (question: AgentQuestion, given: QuestionAnswer, undo?: () => void) => {
      if (!connection) return false;
      setSent((current) => new Set(current).add(question.id));
      setWritingId((current) => (current === question.id ? null : current));
      connection
        .call('answerQuestion', JSON.stringify({ id: question.id, ...given }))
        .catch((error: unknown) => {
          setSent((current) => {
            const next = new Set(current);
            next.delete(question.id);
            return next;
          });
          undo?.();
          onError(problem(error));
        })
        .finally(() => setReads((count) => count + 1));
      return true;
    },
    [connection, onError],
  );

  const write = useCallback((question: AgentQuestion) => {
    setWritingId(question.id);
    setDismissed((current) => {
      if (!current.has(question.id)) return current;
      const next = new Set(current);
      next.delete(question.id);
      return next;
    });
  }, []);
  const stopComposing = useCallback(() => {
    if (!composing) return;
    setWritingId(null);
    setDismissed((current) => new Set(current).add(composing.id));
  }, [composing]);

  return { open, assumed, answered, composing, writing: writing !== null, answer, write, stopComposing };
}
