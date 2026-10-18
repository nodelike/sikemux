import { describe, expect, it } from "vitest";
import type { JiraBoardView, JiraIssueSummary } from "./api";
import { boardPeople, matchingCards, pickKey, unassignedCount } from "./boardFilter";

const card = (key: string, summary: string, assignee: string | null) =>
    ({ key, summary, assignee: assignee ? { accountId: assignee.toLowerCase(), name: assignee } : null }) as JiraIssueSummary;

const board = {
    columns: [
        { name: "To Do", statusIds: [], issues: [card("CIQ-1", "Dossier picker", "Vivek"), card("CIQ-2", "Login page", null)] },
        { name: "Done", statusIds: [], issues: [card("CIQ-3", "Dossier chart", "Kiran"), card("CIQ-4", "Render queue", "Vivek")] },
    ],
} as unknown as JiraBoardView;

describe("boardFilter", () => {
    it("lists everyone with a card, busiest first, and counts the cards nobody has", () => {
        expect(boardPeople(board)).toEqual([
            { accountId: "vivek", name: "Vivek", count: 2 },
            { accountId: "kiran", name: "Kiran", count: 1 },
        ]);
        expect(unassignedCount(board)).toBe(1);
    });

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
