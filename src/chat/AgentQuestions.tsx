import { useContext, type ReactNode } from "react";
import { AgentMark } from "../agents/AgentMark";
import { SECRET_ANSWERED } from "../agents/agentQuestions";
import { useStore } from "../state/store";
import { IconLock } from "../ui/Icons";
import type { AnswerLetter } from "./agentLetters";
import { ChatAgentContext } from "./chatAgent";

/** Who is asking, above the question itself. */
export function AskedBy({ cli, ticket, children }: { cli: string; ticket?: string; children: ReactNode }) {
    return (
        <div className="chat-ask-who">
            <AgentMark cli={cli} size={16} />
            <span>{children}</span>
            {ticket && <span className="chat-ask-ticket">{ticket}</span>}
        </div>
    );
}

/**
 * A question as it reads once answered: its words, and the answer in one line, or only that it was answered when the
 * answer is a secret. Who asked is said only when it was not this chat's own agent.
 */
export function AnsweredQuestion({ letter }: { letter: AnswerLetter }) {
    const chatId = useContext(ChatAgentContext).id;
    const own = useStore((s) => s.agents[chatId]?.name === letter.asker);
    return (
        <>
            {!own && (
                <AskedBy cli={letter.cli}>
                    <b>{letter.asker}</b> asked you
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
                    <span>You answered:</span> <span className="chat-ask-answer">{letter.text}</span>
                </div>
            )}
        </>
    );
}
