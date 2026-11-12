import { useCallback, useEffect, useMemo, useState } from 'react';
import type { ConnectionLike } from '@sikemux/native';

import { couldNotRead } from '@shared/calls/words';
import type { AgentQuestion, AnswerParams, ChatContext } from '@shared/wire.generated';

import { callHost, useHostCall } from '@/core/hostCalls';
import { problem } from '@/core/problem';

/** What the person wrote and what they sent with it, which the host writes out as one text. */
export type WrittenAnswer = { readonly answer: string; readonly paths?: string[]; readonly context?: ChatContext[] };

const NONE: AgentQuestion[] = [];

export type Assumptions = {
  /** Questions the agent went on without, on an assumption it stated. */
  assumed: AgentQuestion[];
  /** The assumption whose answer the composer writes, after Change. */
  changing: AgentQuestion | null;
  change: (question: AgentQuestion) => void;
  stopChanging: () => void;
  /** Tells the agent its assumption holds. */
  fine: (question: AgentQuestion) => void;
  /** Answers an assumption with what the person wrote in the composer, with the files they sent. */
  answer: (question: AgentQuestion, written: WrittenAnswer) => Promise<boolean>;
};

/**
 * The questions an agent went on without, on an assumption it stated, read with the host's `questions` call whenever
 * `seen` changes, as the Mac reads them. Fine lets an assumption stand; Change answers it from the composer.
 */
export function useAssumptions(
  connection: ConnectionLike | undefined,
  agentId: string,
  seen: string,
  onError: (problem: string) => void,
): Assumptions {
  const [sent, setSent] = useState<ReadonlySet<string>>(() => new Set());
  const [changingId, setChangingId] = useState<string | null>(null);
  const [reads, setReads] = useState(0);
  const read = useHostCall(connection, 'questions', {}, `${seen}|${reads}`);
  const all = read.data ?? NONE;
  useEffect(() => {
    if (read.problem) onError(couldNotRead(read.problem));
  }, [read.problem, onError]);

  const assumed = useMemo(
    () => all.filter((question) => question.agentId === agentId && question.state === 'assumed' && !sent.has(question.id)),
    [all, agentId, sent],
  );
  const changing = assumed.find((question) => question.id === changingId) ?? null;

  const send = useCallback(
    async (question: AgentQuestion, params: Omit<AnswerParams, 'id'>) => {
      if (!connection) return false;
      setSent((now) => new Set([...now, question.id]));
      setChangingId((now) => (now === question.id ? null : now));
      try {
        await callHost(connection, 'answerQuestion', { id: question.id, ...params });
        return true;
      } catch (error) {
        setSent((now) => new Set([...now].filter((id) => id !== question.id)));
        onError(problem(error));
        return false;
      } finally {
        setReads((count) => count + 1);
      }
    },
    [connection, onError],
  );

  return {
    assumed,
    changing,
    change: (question) => setChangingId(question.id),
    stopChanging: () => setChangingId(null),
    fine: (question) => void send(question, { fine: true }),
    answer: (question, written) => send(question, { answer: written.answer, paths: written.paths, context: written.context }),
  };
}
