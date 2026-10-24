import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CheckTally, Run } from "../api";

import { ciCore, InHost, registerTestHost, TEST_HOST } from "../testHost";
import { PullChecks } from "./PullChecks";

const host = registerTestHost({});
ciCore();
const repo = { provider: TEST_HOST, owner: "nodelike", name: "sikemux" };

const makeRun = (overrides: Partial<Run> = {}): Run => ({
    id: "7",
    name: "CI",
    title: "Ship it",
    workflowId: "1",
    path: null,
    runNumber: 12,
    attempt: 1,
    event: "pull_request",
    status: "completed",
    conclusion: "success",
    outcome: "success",
    branch: "feat/x",
    sha: "abc",
    shortSha: "abc",
    actor: null,
    avatarUrl: null,
    createdAt: "2026-01-01T12:00:00Z",
    startedAt: "2026-01-01T12:00:00Z",
    updatedAt: "2026-01-01T12:01:30Z",
    pullRequests: [31],
    url: "",
    ...overrides,
});

function renderChecks(checks: Partial<CheckTally> & Pick<CheckTally, "outcome">, onOpenRun = vi.fn()) {
    const tally: CheckTally = { total: 0, passed: 0, failing: 0, running: 0, queued: 0, waiting: 0, cancelled: 0, unknown: 0, ...checks };
    tally.total = tally.passed + tally.failing + tally.running + tally.queued + tally.waiting + tally.cancelled + tally.unknown;
    render(
        <InHost host={host}>
            <PullChecks repo={repo} checks={tally} runs={[{ run: makeRun(), unfinished: false }]} active onOpenRun={onOpenRun} />
        </InHost>,
    );
    return { onOpenRun };
}

const headline = () => document.querySelector(".gha-merge-title")?.textContent;
const overall = () => document.querySelector(".gha-merge-row [role=img]")?.getAttribute("aria-label");

afterEach(() => {
    cleanup();
    vi.useRealTimers();
});

describe("PullChecks", () => {
    it("says all passed when every run passed or was skipped", () => {
        renderChecks({ outcome: "success", passed: 2 });
        expect(headline()).toBe("All checks passed");
        expect(overall()).toBe("Passed");
    });

    it("leads with a failure over anything still going", () => {
        renderChecks({ outcome: "failure", failing: 1, running: 1, passed: 1 });
        expect(headline()).toBe("Checks: 1 failing, 1 running, 1 passed");
        expect(overall()).toBe("Failed");
    });

    it("reads as waiting when a run is held for approval and nothing else is going", () => {
        renderChecks({ outcome: "blocked", waiting: 1, passed: 1 });
        expect(headline()).toBe("Checks: 1 waiting, 1 passed");
        expect(overall()).toBe("Waiting for approval");
    });
});
