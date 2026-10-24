import { describe, expect, it } from "vitest";
import type { JiraColumn, JiraIssueSummary } from "./api";
import { matchingCards, pickKey } from "./boardFilter";

const card = (key: string, summary: string, assignee: string | null) =>
    ({ key, summary, assignee: assignee ? { accountId: assignee.toLowerCase(), name: assignee } : null }) as JiraIssueSummary;

const board: { columns: JiraColumn[] } = {
    columns: [
        { name: "To Do", statusIds: [], issues: [card("CIQ-1", "Dossier picker", "Vivek"), card("CIQ-2", "Login page", null)] },
        { name: "Done", statusIds: [], issues: [card("CIQ-3", "Dossier chart", "Kiran"), card("CIQ-4", "Render queue", "Vivek")] },
    ],
};

describe("boardFilter", () => {
    it("makes the same key for the same picks in any order", () => {
        expect(pickKey(new Set(["me", "abc"]))).toBe(pickKey(new Set(["abc", "me"])));
        expect(pickKey(new Set())).toBe("");
    });

    it("keeps the cards whose key or title holds the words", () => {
        const keys = (words: string) => matchingCards(board.columns, words).map((column) => column.issues.map((issue) => issue.key));
        expect(keys("dossier")).toEqual([["CIQ-1"], ["CIQ-3"]]);
        expect(keys("ciq-4")).toEqual([[], ["CIQ-4"]]);
        expect(matchingCards(board.columns, "  ")).toBe(board.columns);
    });
});
