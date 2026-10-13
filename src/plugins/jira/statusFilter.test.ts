import { describe, expect, it } from "vitest";
import type { JiraIssueSummary, StatusCategory } from "./api";
import { statusCounts, withStatuses } from "./statusFilter";

const issue = (key: string, status: string, statusCategory: StatusCategory) => ({ key, status, statusCategory }) as JiraIssueSummary;

const issues = [
    issue("CIQ-1", "Deployed", "done"),
    issue("CIQ-2", "To Do", "new"),
    issue("CIQ-3", "In Progress", "indeterminate"),
    issue("CIQ-4", "Deployed", "done"),
    issue("CIQ-5", "Done", "done"),
];

describe("statusCounts", () => {
    it("counts each status, work in progress first and finished work last", () => {
        expect(statusCounts(issues)).toEqual([
            { status: "In Progress", category: "indeterminate", count: 1 },
            { status: "To Do", category: "new", count: 1 },
            { status: "Deployed", category: "done", count: 2 },
            { status: "Done", category: "done", count: 1 },
        ]);
    });
});

describe("withStatuses", () => {
    it("keeps every issue when no status is picked", () => {
        expect(withStatuses(issues, new Set())).toBe(issues);
    });

    it("keeps the issues in any picked status", () => {
        expect(withStatuses(issues, new Set(["Deployed", "To Do"])).map((each) => each.key)).toEqual(["CIQ-1", "CIQ-2", "CIQ-4"]);
    });
});
