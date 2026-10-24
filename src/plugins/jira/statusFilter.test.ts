import { describe, expect, it } from "vitest";
import type { JiraIssueSummary } from "./api";
import { withStatuses } from "./statusFilter";

const issue = (key: string, status: string, statusCategory: string) => ({ key, status, statusCategory }) as JiraIssueSummary;

const issues = [
    issue("CIQ-1", "Deployed", "done"),
    issue("CIQ-2", "To Do", "new"),
    issue("CIQ-3", "In Progress", "indeterminate"),
    issue("CIQ-4", "Deployed", "done"),
    issue("CIQ-5", "Done", "done"),
];

describe("withStatuses", () => {
    it("keeps every issue when no status is picked", () => {
        expect(withStatuses(issues, new Set())).toBe(issues);
    });

    it("keeps the issues in any picked status", () => {
        expect(withStatuses(issues, new Set(["Deployed", "To Do"])).map((each) => each.key)).toEqual(["CIQ-1", "CIQ-2", "CIQ-4"]);
    });
});
