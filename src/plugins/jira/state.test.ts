import { renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { jqlOf, mentions, searchJql, updateJiraView, useJiraView } from "./state";

describe("the Jira pane's lists", () => {
    it("lists a project's open and unassigned issues, keeping only what a project key can hold", () => {
        expect(jqlOf({ kind: "project", key: "CIQ", name: "ChannelIQ" })).toBe('project = "CIQ" AND statusCategory != Done ORDER BY updated DESC');
        expect(jqlOf({ kind: "unassigned", key: 'CIQ" OR 1=1', name: "x" })).toBe(
            'project = "CIQOR11" AND assignee is EMPTY AND statusCategory != Done ORDER BY created DESC',
        );
    });

    it("remembers each pane's list and open issue on its own, starting on my issues", () => {
        updateJiraView("pane-a", { issue: "ABC-1" });
        updateJiraView("pane-b", { list: { kind: "sprint" } });
        const a = renderHook(() => useJiraView("pane-a")).result.current;
        const b = renderHook(() => useJiraView("pane-b")).result.current;
        const fresh = renderHook(() => useJiraView("pane-c")).result.current;
        expect(a).toEqual({ list: { kind: "mine" }, issue: "ABC-1", site: "" });
        expect(b).toEqual({ list: { kind: "sprint" }, issue: null, site: "" });
        expect(fresh.list).toEqual({ kind: "mine" });
    });
});

describe("commits that mention an issue", () => {
    it("matches the key as a whole word in any case", () => {
        expect(mentions("ABC-12: fix the race", "ABC-12")).toBe(true);
        expect(mentions("fix(auth): handle expiry (abc-12)", "ABC-12")).toBe(true);
        expect(mentions("feature/abc-12-login", "ABC-12")).toBe(true);
        expect(mentions("ABC-123 is something else", "ABC-12")).toBe(false);
        expect(mentions("XABC-12", "ABC-12")).toBe(false);
    });
});

describe("the search box", () => {
    it("runs JQL as it is, and finds plain words in issue text", () => {
        expect(searchJql("project = ABC AND status = 'To Do'")).toBe("project = ABC AND status = 'To Do'");
        expect(searchJql("assignee in (currentUser()) order by created")).toBe("assignee in (currentUser()) order by created");
        expect(searchJql(" ankit ")).toBe('text ~ "ankit" ORDER BY updated DESC');
        expect(searchJql('login "race"')).toBe('text ~ "login \\"race\\"" ORDER BY updated DESC');
        expect(jqlOf({ kind: "jql", jql: "login bug" })).toBe('text ~ "login bug" ORDER BY updated DESC');
        expect(searchJql("bug in login is slow")).toBe('text ~ "bug in login is slow" ORDER BY updated DESC');
        expect(searchJql("assignee is empty")).toBe("assignee is empty");
    });
});
