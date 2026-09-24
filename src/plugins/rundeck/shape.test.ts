import { describe, expect, it } from "vitest";
import {
    branchOf,
    childSegment,
    displayStatus,
    envOf,
    envTone,
    inGroup,
    isLiveStatus,
    isProdEnv,
    isProdTarget,
    localDateTimeToIso,
    qualifiedName,
    relativeTime,
    topFolder,
} from "./shape";

const PROD = ["prod", "production", "prd", "live"];

describe("branchOf", () => {
    it("takes the first configured option, case-insensitively and in order", () => {
        const options = { git_ref: "tag-1", Branch: "feature/x" };
        expect(branchOf(options, ["BRANCH", "GIT_REF"])).toBe("feature/x");
        expect(branchOf(options, ["GIT_REF", "BRANCH"])).toBe("tag-1");
        expect(branchOf(options, ["REF"])).toBeNull();
        expect(branchOf(null, ["BRANCH"])).toBeNull();
        expect(branchOf({ BRANCH: "" }, ["BRANCH"])).toBeNull();
    });
});

describe("groups", () => {
    it("reads the top folder of any non-empty group in its original case", () => {
        expect(topFolder("Prod/service-a")).toBe("Prod");
        expect(topFolder("staging")).toBe("staging");
        expect(topFolder("")).toBeNull();
        expect(topFolder(null)).toBeNull();
        expect(envOf("Deployments", "Staging/api")).toBe("Staging");
        expect(envOf("Deployments", null)).toBe("Deployments");
    });

    it("matches a group and everything below it", () => {
        expect(inGroup("prod/backend/api", "prod")).toBe(true);
        expect(inGroup("prod", "prod")).toBe(true);
        expect(inGroup("production/api", "prod")).toBe(false);
        expect(inGroup(null, null)).toBe(true);
        expect(inGroup(null, "prod")).toBe(false);
        expect(childSegment("prod/backend/api", "prod")).toBe("backend");
        expect(childSegment("prod", "prod")).toBeNull();
        expect(childSegment("prod/backend", null)).toBe("prod");
        expect(qualifiedName("api", "prod/backend")).toBe("prod/backend/api");
        expect(qualifiedName("api", null)).toBe("api");
    });
});

describe("production", () => {
    it("matches whole tokens, not prefixes", () => {
        expect(isProdEnv("prod", PROD)).toBe(true);
        expect(isProdEnv("PROD", PROD)).toBe(true);
        expect(isProdEnv("eu-prod", PROD)).toBe(true);
        expect(isProdEnv("live_payments", PROD)).toBe(true);
        expect(isProdEnv("production", PROD)).toBe(true);
        expect(isProdEnv("preprod", PROD)).toBe(false);
        expect(isProdEnv("product-catalog", PROD)).toBe(false);
        expect(isProdEnv("delivery", PROD)).toBe(false);
    });

    it("checks both the top folder and the project", () => {
        expect(isProdTarget("Deployments", "prod/api", PROD)).toBe(true);
        expect(isProdTarget("payments-prod", "backend/api", PROD)).toBe(true);
        expect(isProdTarget("Deployments", "staging/api", PROD)).toBe(false);
        expect(isProdTarget("Deployments", "staging/api", ["staging"])).toBe(true);
    });

    it("colours non-production environments", () => {
        expect(envTone("staging")).toBe("staging");
        expect(envTone("pre-prod")).toBe("preprod");
        expect(envTone("dev")).toBe("dev");
        expect(envTone("ops")).toBe("other");
    });
});

describe("status", () => {
    it("treats queued and scheduled runs as live", () => {
        expect(isLiveStatus("running")).toBe(true);
        expect(isLiveStatus("Scheduled")).toBe(true);
        expect(isLiveStatus("queued")).toBe(true);
        expect(isLiveStatus("succeeded")).toBe(false);
        expect(isLiveStatus(null)).toBe(false);
    });

    it("shows a custom status in place of other", () => {
        expect(displayStatus("other", "deployed-with-warnings")).toBe("deployed-with-warnings");
        expect(displayStatus("failed", "ignored")).toBe("failed");
        expect(displayStatus(null, null)).toBe("unknown");
    });
});

describe("time", () => {
    it("calls anything under five seconds, or in the future, just now", () => {
        const now = Date.parse("2026-09-25T12:00:00Z");
        expect(relativeTime("2026-09-25T12:00:03Z", now)).toBe("just now");
        expect(relativeTime("2026-09-25T11:59:57Z", now)).toBe("just now");
        expect(relativeTime("2026-09-25T11:59:30Z", now)).toBe("30s ago");
        expect(relativeTime("2026-09-25T11:00:00Z", now)).toBe("1h ago");
    });

    it("turns a datetime-local value into ISO with the local offset", () => {
        const iso = localDateTimeToIso("2026-09-25T14:30");
        expect(iso).toMatch(/^2026-09-25T14:30:00[+-]\d{2}:\d{2}$/);
        expect(Date.parse(iso!)).toBe(new Date(2026, 8, 25, 14, 30).getTime());
        expect(localDateTimeToIso("not a date")).toBeNull();
    });
});
