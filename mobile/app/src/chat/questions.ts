import { useCallback, useEffect, useMemo, useState } from 'react';
import type { ConnectionLike } from '@sikemux/native';

import { unreadAnswers, waitingQuestions, type AgentQuestion, type QuestionAnswer, type SentAnswer } from '@mac/agents/agentQuestions';
import type { ChatMessage } from '@mac/chat/types';

import { useHostCall } from '@/core/hostCalls';
import { problem } from '@/core/problem';

const NONE: AgentQuestion[] = [];

export type ChatQuestions = {
  /** What the agent asked the person and waits on, oldest first. */
  open: AgentQuestion[];
  /** Questions the agent went on without, on an assumption. */
  assumed: AgentQuestion[];
  /** Answered from this phone, until the transcript holds the answer. */
  answered: { question: AgentQuestion; answer: string; skipped: boolean }[];
  busy: string | null;
  /**
   * The question whatever the composer sends answers, while one is open and the person has not taken the chip off; never
   * a secret one, which has its own masked field.
   */
  composing: AgentQuestion | null;
  /** The person chose to write the answer. */
  writing: boolean;
  /** `shown` is what the transcript shows as the answer until the agent has it; none for an assumption let stand. */
  answer: (question: AgentQuestion, given: QuestionAnswer, shown: string | null) => Promise<boolean>;
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
  messages: readonly ChatMessage[],
  onError: (problem: string) => void,
): ChatQuestions {
  const [busy, setBusy] = useState<string | null>(null);
  const [sent, setSent] = useState<SentAnswer[]>([]);
  const [writingId, setWritingId] = useState<string | null>(null);
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(() => new Set());

  const [reads, setReads] = useState(0);
  const read = useHostCall<AgentQuestion[]>(connection, 'questions', null, `${seen}|${reads}`);
  const all = read.data ?? NONE;
  useEffect(() => {
    if (read.problem) onError(`could not read this agent's questions: ${read.problem}`);
  }, [read.problem, onError]);

  const { open, assumed } = useMemo(() => waitingQuestions(all, agentId, sent), [all, agentId, sent]);
  const answered = useMemo(() => unreadAnswers(sent, messages), [sent, messages]);
  const writing = [...open, ...assumed].find((question) => question.id === writingId && !question.secret) ?? null;
  const composing = writing ?? open.find((question) => !question.secret && !dismissed.has(question.id)) ?? null;

  const answer = useCallback(
    async (question: AgentQuestion, given: QuestionAnswer, shown: string | null) => {
      if (!connection) return false;
      setBusy(question.id);
      try {
        await connection.call('answerQuestion', JSON.stringify({ id: question.id, ...given }));
        setSent((current) => [...current, { question, answer: shown, skipped: false }]);
        setWritingId((current) => (current === question.id ? null : current));
        return true;
      } catch (error) {
        onError(problem(error));
        setReads((count) => count + 1);
        return false;
      } finally {
        setBusy(null);
      }
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

  return { open, assumed, answered, busy, composing, writing: writing !== null, answer, write, stopComposing };
}
