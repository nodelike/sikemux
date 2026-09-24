import { describe, expect, it } from "vitest";
import { BRANCH_GLYPH, branchKind, statusKind } from "./branchStyle";

describe("Rundeck branch/status styling logic", () => {
    it("classifies branch names", () => {
        expect(branchKind(null)).toBe("na");
        expect(branchKind("origin/main")).toBe("main");
        expect(branchKind("feature/login")).toBe("feature");
        expect(branchKind("hotfix/prod")).toBe("fix");
        expect(branchKind("release/1.2.3")).toBe("release");
        expect(branchKind("experiment/x")).toBe("other");
        expect(BRANCH_GLYPH.feature).toBe("◆");
    });

    it("classifies Rundeck execution statuses", () => {
        expect(statusKind("succeeded")).toBe("succeeded");
        expect(statusKind("timedout")).toBe("failed");
        expect(statusKind("running")).toBe("running");
        expect(statusKind("queued")).toBe("running");
        expect(statusKind("failed-with-retry")).toBe("failed");
        expect(statusKind("aborted")).toBe("aborted");
        expect(statusKind(undefined)).toBe("unknown");
    });
});
