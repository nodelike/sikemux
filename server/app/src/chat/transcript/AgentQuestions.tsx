import { AgentMark } from "../agents/AgentMark";
import { IconLock } from "@shared/ui/Icons";
import { SECRET_ANSWERED } from "@shared/chat/agentLetters";
import type { AgentLetter } from "@shared/wire.generated";

/** A question as it reads once answered: who asked, its words, and the answer in one line, or only that it was answered when the answer is a secret. */
export function AnsweredQuestion({ letter }: { letter: AgentLetter }) {
  return (
    <>
      <div className="chat-ask-who">
        <AgentMark type={letter.cli} size={16} />
        <span>
          <b>{letter.name}</b> asked you
        </span>
      </div>
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
