import { useEffect, useState } from 'react';
import type { ConnectionLike } from '@sikemux/native';

import type { AgentQuestion } from '@mac/agents/agentQuestions';

const NONE: AgentQuestion[] = [];

/**
 * Every question the host's agents put to the person or their leads, read with its `questions` call whenever `seen`
 * changes. A host that cannot answer has none, and the chat that needs them says why.
 */
export function useHostQuestions(connection: ConnectionLike | undefined, seen: string): AgentQuestion[] {
  const [questions, setQuestions] = useState(NONE);
  useEffect(() => {
    if (!connection) return;
    let current = true;
    connection.call('questions', 'null').then(
      (text) => {
        if (current) setQuestions(JSON.parse(text) as AgentQuestion[]);
      },
      () => {
        if (current) setQuestions(NONE);
      },
    );
    return () => {
      current = false;
    };
  }, [connection, seen]);
  return questions;
}
