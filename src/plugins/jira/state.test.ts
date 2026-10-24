import { renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { mentions, updateJiraView, useJiraView } from "./state";

describe("the Jira pane's lists", () => {
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
