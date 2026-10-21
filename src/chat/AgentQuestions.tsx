import { useContext, type ReactNode } from "react";
import { AgentMark } from "../agents/AgentMark";
import { SECRET_ANSWERED, SKIPPED } from "../agents/agentQuestions";
import { useStore } from "../state/store";
import { IconClose, IconLock } from "../ui/Icons";
import type { AnswerLetter } from "./agentLetters";
import { ChatAgentContext } from "./chatAgent";

/** Who is asking, above the question itself. */
export function AskedBy({ agentId, cli, ticket, children }: { agentId?: string; cli: string; ticket?: string; children: ReactNode }) {
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
export function AnsweredQuestion({ letter }: { letter: AnswerLetter }) {
    const chatId = useContext(ChatAgentContext).id;
    const own = useStore((s) => s.agents[chatId]?.name === letter.asker);
    const askerId = useStore((s) => Object.values(s.agents).find((agent) => agent.name === letter.asker)?.id);
    return (
        <>
            {!own && (
                <AskedBy agentId={askerId} cli={letter.cli}>
                    <b>{letter.asker}</b> asked you
                </AskedBy>
            )}
            <div className="chat-ask-text">{letter.question}</div>
            {letter.skipped ? (
                <div className="chat-ask-answered skipped">
                    <IconClose size={12} />
                    <span>{SKIPPED}</span>
                </div>
            ) : letter.secret ? (
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
