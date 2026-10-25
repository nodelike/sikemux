import { useContext, type ReactNode } from "react";
import { AgentMark } from "./agents/AgentMark";
import { IconLock } from "@shared/ui/Icons";
import { SECRET_ANSWERED } from "@shared/chat/agentLetters";
import type { AgentLetter } from "@shared/chat/types";
import { ChatAgentContext } from "./chatAgent";

/** Who is asking, above the question itself. */
export function AskedBy({
  agentId,
  cli,
  ticket,
  children,
}: {
  agentId?: string;
  cli: string;
  ticket?: string;
  children: ReactNode;
}) {
  return (
    <div className="chat-ask-who">
      <AgentMark agentId={agentId} cli={cli} size={16} />
      <span>{children}</span>
      {ticket && <span className="chat-ask-ticket">{ticket}</span>}
    </div>
  );
}

/**
 * A question as it reads once answered: its words, and the answer in one line, or only that it was answered when the
 * answer is a secret. Who asked is said only when it was not this chat's own agent.
 */
export function AnsweredQuestion({ letter }: { letter: AgentLetter }) {
  const chatId = useContext(ChatAgentContext).id;
  const own = letter.agentId === chatId;
  const askerId = letter.agentId || undefined;
  return (
    <>
      {!own && (
        <AskedBy agentId={askerId} cli={letter.cli}>
          <b>{letter.name}</b> asked you
        </AskedBy>
      )}
      <div className="chat-ask-text">{letter.question}</div>
      {letter.secret ? (
        <div className="chat-ask-answered secret">
          <IconLock size={12} />
          <span>{SECRET_ANSWERED}</span>
        </div>
      ) : (
        <div className="chat-ask-answered">
          <span>You answered:</span>{" "}
          <span className="chat-ask-answer">{letter.text}</span>
        </div>
      )}
    </>
  );
}
