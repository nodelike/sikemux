import { useCallback, useMemo, useState } from "react";
import { errorText } from "../lib/format";
import { agentQuestionsApi, type AgentQuestion, type QuestionAnswer } from "../api/agentQuestions";
import { useStore } from "../state/store";

const NO_PICKS: ReadonlyMap<string, QuestionPick> = new Map();
const NONE: ReadonlySet<string> = new Set();

/** An answer picked for an open question, held until every open question has one. */
export interface QuestionPick {
    answer: QuestionAnswer;
    /** What the chat shows was answered; empty for a secret. */
    shown: string;
}

export interface ChatQuestions {
    /** What the agent asked the person and waits on, oldest first. */
    open: AgentQuestion[];
    /** Questions the agent went on without, on an assumption. */
    assumed: AgentQuestion[];
    /** Answered, until the agent has read the answer. */
    answered: AgentQuestion[];
    /** The assumed question whose answer is on its way. */
    busy: string | null;
    /** The open questions' answers are on their way. */
    sending: boolean;
    picks: ReadonlyMap<string, QuestionPick>;
    /** Every open question has an answer picked. */
    ready: boolean;
    /** The open question on show. */
    current: AgentQuestion | null;
    /** The choice of `current` the arrow keys moved to, if they have. */
    cursor: number | null;
    /** The open questions are folded away, and the composer writes to the agent again, until another question comes first. */
    folded: boolean;
    /** The question whatever is sent from the composer answers; never a secret one, which has its own masked field. */
    composing: AgentQuestion | null;
    /** The composer answers an assumed question because the person chose to change it. */
    writing: boolean;
    answer: (question: AgentQuestion, answer: QuestionAnswer) => Promise<boolean>;
    pick: (question: AgentQuestion, pick: QuestionPick) => void;
    /** Picks a choice, or ticks it on a question that takes several. */
    choose: (question: AgentQuestion, choice: number) => void;
    show: (question: AgentQuestion) => void;
    fold: (folded: boolean) => void;
    submit: () => void;
    write: (question: AgentQuestion) => void;
    stopComposing: () => void;
    /** A key pressed while nothing is typed: numbers pick, arrows move, Space ticks, Enter picks, moves on or sends. True when it did something. */
    key: (key: string, primary: boolean) => boolean;
}

const choicePick = (question: AgentQuestion, choice: number): QuestionPick => ({
    answer: { choice },
    shown: question.choices[choice]?.label ?? "",
});

/** The questions an agent put to the person, answered in its chat. */
export function useChatQuestions(agentId: string, onError: (message: string | null) => void): ChatQuestions {
    const all = useStore((s) => s.agentQuestions);
    const [busy, setBusy] = useState<string | null>(null);
    const [sending, setSending] = useState(false);
    /* Answered from here, kept out of sight until the core's next list moves them on. */
    const [sent, setSent] = useState(NONE);
    const { open, assumed, answered } = useMemo(() => {
        const own = all.filter((question) => question.agentId === agentId);
        const waiting = (state: AgentQuestion["state"]) => own.filter((question) => question.state === state && !sent.has(question.id));
        return { open: waiting("open"), assumed: waiting("assumed"), answered: own.filter((question) => question.state === "answered") };
    }, [all, agentId, sent]);
    const [writingId, setWritingId] = useState<string | null>(null);
    const [heldPicks, setPicks] = useState<ReadonlyMap<string, QuestionPick>>(NO_PICKS);
    const [shownId, setShownId] = useState<string | null>(null);
    const [cursorAt, setCursor] = useState<{ id: string; at: number } | null>(null);
    const [foldedAt, setFoldedAt] = useState<string | null>(null);
    const folded = open.length > 0 && foldedAt === open[0].id;

    const picks = useMemo(() => {
        if (heldPicks.size === 0) return heldPicks;
        const live = new Map([...heldPicks].filter(([id]) => open.some((question) => question.id === id)));
        return live.size === heldPicks.size ? heldPicks : live;
    }, [heldPicks, open]);
    const ready = open.length > 0 && open.every((question) => picks.has(question.id));
    const current = open.find((question) => question.id === shownId) ?? open.find((question) => !picks.has(question.id)) ?? open[0] ?? null;
    const picked = current ? picks.get(current.id) : undefined;
    const moved = current && cursorAt?.id === current.id ? cursorAt.at : null;
    const cursor = moved ?? (picked && "choice" in picked.answer ? picked.answer.choice : 0);

    const writing = assumed.find((question) => question.id === writingId) ?? null;
    const composing = writing ?? (!folded && current && !current.secret ? current : null);

    const answer = useCallback(
        async (question: AgentQuestion, given: QuestionAnswer) => {
            setBusy(question.id);
            onError(null);
            try {
                await agentQuestionsApi.answer(question.id, given);
                setSent((current) => new Set(current).add(question.id));
                setWritingId((current) => (current === question.id ? null : current));
                return true;
            } catch (error) {
                onError(errorText(error));
                return false;
            } finally {
                setBusy(null);
            }
        },
        [onError],
    );

    const send = useCallback(
        (given: ReadonlyMap<string, QuestionPick>) => {
            const answers = open.flatMap((question) => {
                const pick = given.get(question.id);
                return pick ? [{ question, pick }] : [];
            });
            if (sending || answers.length !== open.length || answers.length === 0) return;
            setSending(true);
            onError(null);
            agentQuestionsApi
                .answerAll(answers.map(({ question, pick }) => ({ id: question.id, answer: pick.answer })))
                .then(() => {
                    setSent((current) => new Set([...current, ...answers.map(({ question }) => question.id)]));
                    setPicks(NO_PICKS);
                    setShownId(null);
                    setCursor(null);
                })
                .catch((error: unknown) => onError(errorText(error)))
                .finally(() => setSending(false));
        },
        [onError, open, sending],
    );

    /** Moves on from `question` once it has its answer: to the next question still without one, or sends a lone question. */
    const moveOn = useCallback(
        (question: AgentQuestion, given: ReadonlyMap<string, QuestionPick>) => {
            setCursor(null);
            if (open.length === 1) {
                send(given);
                return;
            }
            const following =
                open.slice(open.indexOf(question) + 1).find((other) => !given.has(other.id)) ?? open.find((other) => !given.has(other.id));
            if (following) setShownId(following.id);
        },
        [open, send],
    );

    const pick = useCallback(
        (question: AgentQuestion, given: QuestionPick) => {
            const next = new Map(picks).set(question.id, given);
            setPicks(next);
            moveOn(question, next);
        },
        [moveOn, picks],
    );

    /** Ticks or unticks a choice of a question that takes several, staying on it. */
    const toggle = useCallback(
        (question: AgentQuestion, choice: number) => {
            const held = picks.get(question.id)?.answer;
            const ticked = held && "choices" in held ? held.choices : [];
            const choices = ticked.includes(choice) ? ticked.filter((other) => other !== choice) : [...ticked, choice].sort((a, b) => a - b);
            const next = new Map(picks);
            if (choices.length === 0) next.delete(question.id);
            else next.set(question.id, { answer: { choices }, shown: choices.map((index) => question.choices[index]?.label ?? "").join(", ") });
            setPicks(next);
            setCursor({ id: question.id, at: choice });
        },
        [picks],
    );

    const choose = useCallback(
        (question: AgentQuestion, choice: number) => (question.multi ? toggle(question, choice) : pick(question, choicePick(question, choice))),
        [pick, toggle],
    );
    const show = useCallback((question: AgentQuestion) => {
        setShownId(question.id);
        setCursor(null);
    }, []);
    const submit = useCallback(() => send(picks), [picks, send]);
    const firstOpen = open[0]?.id ?? null;
    const fold = useCallback((fold: boolean) => setFoldedAt(fold ? firstOpen : null), [firstOpen]);

    const key = (key: string, primary: boolean): boolean => {
        if (!current || folded || sending) return false;
        if (primary) {
            if (key !== "Enter" || !ready) return false;
            submit();
            return true;
        }
        const count = current.choices.length;
        if (/^[1-9]$/.test(key)) {
            if (Number(key) > count) return false;
            choose(current, Number(key) - 1);
            return true;
        }
        if (key === "ArrowUp" || key === "ArrowDown") {
            if (count === 0) return false;
            setCursor({ id: current.id, at: (cursor + (key === "ArrowUp" ? count - 1 : 1)) % count });
            return true;
        }
        if (key === "ArrowLeft" || key === "ArrowRight") {
            if (open.length < 2) return false;
            const at = open.indexOf(current) + (key === "ArrowLeft" ? -1 : 1);
            show(open[(at + open.length) % open.length]);
            return true;
        }
        if (key === " " && current.multi) {
            toggle(current, cursor);
            return true;
        }
        if (key !== "Enter") return false;
        if (current.multi) {
            if (!picked) toggle(current, cursor);
            else if (ready) submit();
            else moveOn(current, picks);
            return true;
        }
        const already = picked && "choice" in picked.answer && picked.answer.choice === cursor;
        if (ready && (already || count === 0)) submit();
        else if (count > 0) choose(current, cursor);
        else return false;
        return true;
    };

    const write = useCallback((question: AgentQuestion) => setWritingId(question.id), []);
    const stopComposing = useCallback(() => setWritingId(null), []);

    return {
        open,
        assumed,
        answered,
        busy,
        sending,
        picks,
        ready,
        current,
        cursor: moved,
        folded,
        composing,
        writing: writing !== null,
        answer,
        pick,
        choose,
        show,
        fold,
        submit,
        write,
        stopComposing,
        key,
    };
}
