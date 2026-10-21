import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentQuestion } from "../agents/agentQuestions";
import { agentQuestionsApi } from "../api/agentQuestions";
import { useStore } from "../state/store";
import { useChatQuestions } from "./useChatQuestions";

vi.mock("../api/agentQuestions", () => ({ agentQuestionsApi: { answer: vi.fn(), answerAll: vi.fn() } }));

const question = (id: string): AgentQuestion => ({
    id,
    agentId: "agent-1",
    name: "Maple",
    cli: "claude",
    project: "/p",
    asked: `${id}?`,
    text: `${id}?`,
    choices: [{ label: "Yes" }],
    blocked: true,
    assumption: null,
    secret: false,
    at: 0,
    lead: null,
    via: null,
    passed: false,
});

describe("useChatQuestions", () => {
    afterEach(() => useStore.setState({ agentQuestions: [] }));

    it("skips every open question at once and shows each as skipped until the agent has them", async () => {
        vi.mocked(agentQuestionsApi.answerAll).mockResolvedValue({ answered: true });
        useStore.setState({ agentQuestions: [question("q1"), question("q2")] });
        const onError = vi.fn();
        const { result } = renderHook(() => useChatQuestions("agent-1", [], onError));
        act(() => result.current.skip());
        await waitFor(() => expect(result.current.open).toEqual([]));
        expect(agentQuestionsApi.answerAll).toHaveBeenCalledWith([
            { id: "q1", answer: { skip: true } },
            { id: "q2", answer: { skip: true } },
        ]);
        expect(result.current.answered.map(({ question, skipped }) => [question.id, skipped])).toEqual([
            ["q1", true],
            ["q2", true],
        ]);
        expect(onError).toHaveBeenLastCalledWith(null);
    });
});
