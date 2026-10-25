import type { AgentQuestion } from "../api/agentQuestions";
import { IconCheck } from "@shared/ui/Icons";
import { AnsweredQuestion } from "./AgentQuestions";
import type { ChatQuestions } from "./useChatQuestions";

/** A question the agent went on without, on the assumption it stated. */
function AssumedQuestion({ question, busy, onFine, onChange }: { question: AgentQuestion; busy: boolean; onFine: () => void; onChange: () => void }) {
    return (
        <div className="chat-message agent-question">
            <div className="chat-message-content">
                <div className="chat-ask-assumed" title={question.text}>
                    <IconCheck size={14} />
                    <span>
                        <b>{question.name}</b> assumed {question.assumption}
                    </span>
                    <span className="chat-ask-dot">·</span>
                    <button type="button" disabled={busy} onClick={onFine}>
                        Fine
                    </button>
                    <button type="button" disabled={busy} onClick={onChange}>
                        Change
                    </button>
                </div>
            </div>
        </div>
    );
}

/** What the agent asked at the foot of its chat: questions answered here until the transcript holds them, and those it went on without. */
export default function QuestionRows({ questions, onWrite }: { questions: ChatQuestions; onWrite: (question: AgentQuestion) => void }) {
    return (
        <>
            {questions.answered.map((question) => (
                <article key={question.id} className="chat-message agent-question">
                    <div className="chat-message-content">
                        <AnsweredQuestion
                            letter={{
                                kind: "answer",
                                name: question.name,
                                agentId: question.agentId,
                                cli: question.cli,
                                question: question.text,
                                text: question.answer ?? "",
                                secret: question.secret,
                            }}
                        />
                    </div>
                </article>
            ))}
            {questions.assumed.map((question) => (
                <AssumedQuestion
                    key={question.id}
                    question={question}
                    busy={questions.busy === question.id}
                    onFine={() => void questions.answer(question, { fine: true })}
                    onChange={() => onWrite(question)}
                />
            ))}
        </>
    );
}
