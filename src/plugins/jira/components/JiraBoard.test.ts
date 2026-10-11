import { describe, expect, it } from "vitest";
import type { JiraBoardView, JiraIssueSummary } from "../api";
import { sprintLeft, withMoves } from "./JiraBoard";

const card = (key: string): JiraIssueSummary => ({
    key,
    summary: key,
    status: "",
    statusCategory: "",
    priority: null,
    assignee: null,
    issueType: null,
    sprint: null,
    updated: null,
    url: "",
});

const board: JiraBoardView = {
    id: 1,
    name: "B",
    kind: "kanban",
    sprint: null,
    truncated: false,
    columns: [
        { name: "To Do", statusIds: ["1"], issues: [card("A-1"), card("A-2")] },
        { name: "Done", statusIds: ["2"], issues: [card("A-3")] },
    ],
};

describe("the Jira board", () => {
    it("shows a card in the column it is being moved to before Jira answers", () => {
        const columns = withMoves(board, { "A-1": 1 });
        expect(columns.map((column) => column.issues.map((issue) => issue.key))).toEqual([["A-2"], ["A-3", "A-1"]]);
        expect(withMoves(board, {})).toEqual(board.columns);
    });

    it("says how long a sprint has left", () => {
        const now = Date.parse("2026-10-09T09:00:00Z");
        expect(sprintLeft("2026-10-14T09:00:00Z", now)).toBe("5 days left");
        expect(sprintLeft("2026-10-10T10:00:00Z", now)).toBe("1 day left");
        expect(sprintLeft("2026-10-09T18:00:00Z", now)).toBe("ends today");
        expect(sprintLeft("2026-10-09T08:00:00Z", now)).toBe("ended");
        expect(sprintLeft(null, now)).toBeNull();
    });
});
