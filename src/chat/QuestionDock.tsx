import { useEffect, useRef, useState, type RefObject } from "react";
import type { AgentQuestion } from "../api/agentQuestions";
import { PRIMARY_SHORTCUT } from "@shared/lib/platform";
import { IconCheck, IconChevron, IconLock, IconPencil } from "../ui/Icons";
import type { ChatQuestions } from "./useChatQuestions";

/** What the person picks a question by: the agent's own short name for it, or its place. */
export const questionTitle = (question: AgentQuestion, index: number, count: number): string =>
    question.header ?? (count > 1 ? `Question ${index + 1}` : "Question");

const RECOMMENDED = /\s*\(recommended\)\s*$/i;

/** The answer to a secret question, typed into a masked field that keeps nothing once it is picked. */
function SecretAnswer({ question, busy, onPick }: { question: AgentQuestion; busy: boolean; onPick: (text: string) => void }) {
    const [text, setText] = useState("");
    return (
        <form
            className="chat-ask-secret"
            onSubmit={(event) => {
                event.preventDefault();
                if (!text) return;
                onPick(text);
                setText("");
            }}>
            <div className="chat-ask-secret-field">
                <input
                    type="password"
                    name={`secret-${question.id}`}
                    autoComplete="off"
                    autoCorrect="off"
                    autoCapitalize="off"
                    spellCheck={false}
                    aria-label={`Private answer to ${question.name}`}
                    placeholder="Type the answer"
                    value={text}
                    disabled={busy}
                    onChange={(event) => setText(event.target.value)}
                />
                <button type="submit" disabled={busy || !text}>
                    Done
                </button>
            </div>
            <div className="chat-ask-private">
                <IconLock size={12} />
                <span>{question.secretNote}</span>
            </div>
        </form>
    );
}

const isEditable = (target: EventTarget | null): boolean =>
    target instanceof HTMLElement && Boolean(target.closest('input, textarea, select, [contenteditable="true"]'));

/**
 * The questions the agent waits on, stuck to the composer from above. One shows at a time under a tab per question;
 * picking an answer moves on to the next, and nothing reaches the agent until every one has an answer. Whatever is
 * typed in the composer answers the question on show.
 */
export function QuestionDock({
    questions,
    active,
    paneRef,
}: {
    questions: ChatQuestions;
    active: boolean;
    paneRef: RefObject<HTMLDivElement | null>;
}) {
    const keyRef = useRef(questions.key);
    keyRef.current = questions.key;
    const asking = questions.open.length > 0;
    useEffect(() => {
        if (!active || !asking) return;
        const onKey = (event: KeyboardEvent) => {
            if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return;
            if (!/^([1-9]|Arrow(Up|Down|Left|Right))$/.test(event.key)) return;
            const held = document.activeElement;
            if (isEditable(event.target) || (held && held !== document.body && !paneRef.current?.contains(held))) return;
            if (keyRef.current(event.key, false)) event.preventDefault();
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, [active, asking, paneRef]);

    const { open, current, picks, folded, sending } = questions;
    if (!current) return null;
    const count = open.length;
    const word = count === 1 ? "question" : "questions";

    if (folded) {
        return (
            <div className="chat-group is-folded chat-questions" aria-label={`${count} ${word}`}>
                <button type="button" className="chat-group-label chat-group-toggle" aria-expanded={false} onClick={() => questions.fold(false)}>
                    <span>{word}</span>
                    <span className="chat-group-count">{count}</span>
                    <span className="chat-group-peek">{open.map((question, index) => questionTitle(question, index, count)).join(" · ")}</span>
                    <span className="chat-group-chevron">
                        <IconChevron size={10} />
                    </span>
                </button>
            </div>
        );
    }

    const picked = picks.get(current.id);
    const chosen = (index: number) =>
        picked !== undefined &&
        ("choice" in picked.answer ? picked.answer.choice === index : "choices" in picked.answer && picked.answer.choices.includes(index));
    const multi = current.multi === true;
    const typed = picked && "answer" in picked.answer && !current.secret ? picked.shown : null;
    const via = current.viaNote;
    return (
        <div className="chat-group chat-questions" aria-label={`${count} ${word}`}>
            <div className="chat-questions-head">
                <div className="chat-questions-tabs" role="tablist" aria-label="Questions">
                    {open.map((question, index) => (
                        <button
                            key={question.id}
                            type="button"
                            role="tab"
                            aria-selected={question === current}
                            className={`chat-questions-tab${question === current ? " on" : ""}`}
                            onClick={() => questions.show(question)}>
                            <span className="chat-questions-mark">{picks.has(question.id) ? <IconCheck size={10} /> : index + 1}</span>
                            <span className="chat-questions-title">{questionTitle(question, index, count)}</span>
                        </button>
                    ))}
                </div>
                {(count > 1 || multi) && (
                    <button type="button" className="chat-questions-send" disabled={!questions.ready || sending} onClick={questions.submit}>
                        Send
                        <kbd>{PRIMARY_SHORTCUT}↵</kbd>
                    </button>
                )}
                <button type="button" className="chat-questions-fold" aria-label="Fold the questions away" onClick={() => questions.fold(true)}>
                    <IconChevron size={10} />
                </button>
            </div>
            <div className="chat-questions-text">
                {current.text}
                {multi && <span className="chat-questions-via"> Pick any.</span>}
                {via && <span className="chat-questions-via"> · {via}</span>}
            </div>
            {(current.choices.length > 0 || typed) && (
                <div
                    className="chat-questions-choices"
                    role={multi ? "group" : "radiogroup"}
                    aria-label={questionTitle(current, open.indexOf(current), count)}>
                    {current.choices.map((choice, index) => {
                        const recommended = RECOMMENDED.test(choice.label);
                        return (
                            <button
                                key={index}
                                type="button"
                                role={multi ? "checkbox" : "radio"}
                                aria-checked={chosen(index)}
                                className={`chat-questions-choice${chosen(index) ? " chosen" : ""}${questions.cursor === index ? " cursor" : ""}`}
                                disabled={sending}
                                onClick={() => questions.choose(current, index)}>
                                <span className="chat-questions-key">{multi ? <span className="chat-questions-tick" /> : index + 1}</span>
                                <span className="chat-questions-body">
                                    <span className="chat-questions-label">
                                        {recommended ? choice.label.replace(RECOMMENDED, "") : choice.label}
                                        {recommended && <span className="chat-questions-recommended">recommended</span>}
                                    </span>
                                    {choice.detail && <span className="chat-questions-detail">{choice.detail}</span>}
                                </span>
                            </button>
                        );
                    })}
                    {typed && (
                        <div className="chat-questions-choice chosen" role="radio" aria-checked>
                            <span className="chat-questions-key">
                                <IconPencil size={11} />
                            </span>
                            <span className="chat-questions-body">
                                <span className="chat-questions-label">{typed}</span>
                            </span>
                        </div>
                    )}
                </div>
            )}
            {current.secret &&
                (picked ? (
                    <div className="chat-ask-private chat-questions-secret">
                        <IconLock size={12} />
                        <span>Answer kept, sent with the rest</span>
                    </div>
                ) : (
                    <SecretAnswer
                        question={current}
                        busy={sending}
                        onPick={(text) => questions.pick(current, { answer: { answer: text }, shown: "" })}
                    />
                ))}
        </div>
    );
}
