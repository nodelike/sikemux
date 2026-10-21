import { fireEvent, render, screen } from "@testing-library/react";
import { createRef } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentQuestion } from "../agents/agentQuestions";
import { confirmDialog } from "../state/dialog";
import { AnsweredQuestion } from "./AgentQuestions";
import { agentLetters } from "./agentLetters";
import { QuestionDock } from "./QuestionDock";
import type { ChatQuestions } from "./useChatQuestions";

vi.mock("../state/dialog", () => ({ confirmDialog: vi.fn() }));

const question = (id: string, header: string): AgentQuestion => ({
    id,
    agentId: "agent-1",
    name: "Maple",
    cli: "claude",
    project: "/p",
    asked: `${header}?`,
    text: `${header}?`,
    header,
    choices: [{ label: "Yes" }, { label: "No" }],
    blocked: true,
    assumption: null,
    secret: false,
    at: 0,
    lead: null,
    via: null,
    passed: false,
});

function dock(open: AgentQuestion[]): ChatQuestions {
    return {
        open,
        assumed: [],
        answered: [],
        busy: null,
        sending: false,
        picks: new Map(),
        ready: false,
        current: open[0] ?? null,
        cursor: null,
        folded: false,
        composing: open[0] ?? null,
        writing: false,
        answer: vi.fn(),
        pick: vi.fn(),
        choose: vi.fn(),
        show: vi.fn(),
        fold: vi.fn(),
        submit: vi.fn(),
        skip: vi.fn(),
        write: vi.fn(),
        stopComposing: vi.fn(),
        key: vi.fn(() => false),
    };
}

const confirm = vi.mocked(confirmDialog);

describe("skipping an agent's questions", () => {
    beforeEach(() => confirm.mockReset());

    it("skips every open question once the person confirms", async () => {
        confirm.mockResolvedValue(true);
        const questions = dock([question("q1", "Sharing"), question("q2", "Scope")]);
        render(<QuestionDock questions={questions} active paneRef={createRef()} />);
        fireEvent.click(screen.getByRole("button", { name: "Skip the questions" }));
        await vi.waitFor(() => expect(questions.skip).toHaveBeenCalledTimes(1));
        expect(confirm).toHaveBeenCalledWith(expect.objectContaining({ title: "Skip these 2 questions?", confirmLabel: "Skip" }));
        expect(confirm.mock.calls[0]?.[0].body).toContain("Maple is told you skipped them");
    });

    it("sends nothing when the person thinks better of it", async () => {
        confirm.mockResolvedValue(false);
        const questions = dock([question("q1", "Sharing")]);
        render(<QuestionDock questions={questions} active paneRef={createRef()} />);
        fireEvent.click(screen.getByRole("button", { name: "Skip the question" }));
        await vi.waitFor(() => expect(confirm).toHaveBeenCalledWith(expect.objectContaining({ title: "Skip this question?" })));
        expect(questions.skip).not.toHaveBeenCalled();
    });

    it("keeps a skipped question in the chat, marked as skipped", () => {
        const letters = agentLetters(
            '<question-answer asker="Maple" cli="claude" question="Ship it?" note="Stop." skipped="true">\nThe person skipped this question without answering it.\n</question-answer>',
        );
        expect(letters).toEqual([
            {
                kind: "answer",
                asker: "Maple",
                cli: "claude",
                question: "Ship it?",
                text: "The person skipped this question without answering it.",
                secret: false,
                skipped: true,
            },
        ]);
        const letter = letters?.[0];
        if (letter?.kind !== "answer") throw new Error("not an answer");
        render(<AnsweredQuestion letter={letter} />);
        expect(screen.getByText("Ship it?")).toBeInTheDocument();
        expect(screen.getByText("You skipped this question")).toBeInTheDocument();
        expect(screen.queryByText(/You answered/)).not.toBeInTheDocument();
    });
});
