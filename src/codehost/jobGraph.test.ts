import { describe, expect, it } from "vitest";
import type { Job, LogLine, Step } from "./api";
import { graphOf, splitName, stagesOf, stepStarts, type Graph } from "./jobGraph";
import { readJobs } from "./workflowJobs";

function job(id: number, name: string, startedAt: string | null, completedAt: string | null, status = "completed"): Job {
    return {
        id: String(id),
        name,
        status,
        conclusion: status === "completed" ? "success" : null,
        startedAt,
        completedAt,
        runner: null,
        url: null,
        checkRunId: null,
        steps: [],
    };
}

const at = (minute: number) => `2026-09-27T10:${String(minute).padStart(2, "0")}:00Z`;

describe("splitName", () => {
    it("reads the calling workflow off a job's name", () => {
        expect(splitName("Checks / Rust formatting")).toEqual({ group: "Checks", label: "Rust formatting" });
        expect(splitName("Build, verify, and publish")).toEqual({ group: null, label: "Build, verify, and publish" });
        expect(splitName(" / odd")).toEqual({ group: null, label: " / odd" });
    });

    it("does not split inside a matrix job's values", () => {
        expect(splitName("test (ubuntu / node 20)")).toEqual({ group: null, label: "test (ubuntu / node 20)" });
        expect(splitName("Checks / test (a / b)")).toEqual({ group: "Checks", label: "test (a / b)" });
    });
});

describe("stagesOf", () => {
    it("puts a called workflow's jobs in one box and what waited for it after", () => {
        const stages = stagesOf([
            job(1, "Checks / Frontend", at(0), at(3)),
            job(2, "Checks / Rust", at(0), at(3)),
            job(3, "Checks / macOS", at(1), at(7)),
            job(4, "Build, verify, and publish", at(8), at(28)),
        ]);
        expect(stages).toHaveLength(2);
        expect(stages[0]).toHaveLength(1);
        expect(stages[0]?.[0]?.title).toBe("Checks");
        expect(stages[0]?.[0]?.jobs.map((each) => each.label)).toEqual(["Frontend", "Rust", "macOS"]);
        expect(stages[1]?.[0]?.jobs[0]?.label).toBe("Build, verify, and publish");
    });

    it("keeps jobs that overlapped side by side", () => {
        const stages = stagesOf([job(1, "lint", at(0), at(2)), job(2, "test", at(1), at(5))]);
        expect(stages).toHaveLength(1);
        expect(stages[0]).toHaveLength(2);
    });

    it("never starts a stage after one that is still going", () => {
        const stages = stagesOf([job(1, "build", at(0), null, "in_progress"), job(2, "late", at(9), null, "in_progress")]);
        expect(stages).toHaveLength(1);
    });

    it("puts jobs that have not started at the end", () => {
        const stages = stagesOf([job(1, "deploy", null, null, "queued"), job(2, "build", at(0), at(2))]);
        expect(stages.map((stage) => stage.map((group) => group.jobs[0]?.label))).toEqual([["build"], ["deploy"]]);
    });

    it("has nothing to draw for a run with no jobs", () => {
        expect(stagesOf([])).toEqual([]);
    });

    it("counts a job with an unreadable start as not started", () => {
        const stages = stagesOf([job(1, "odd", "yesterday", null, "queued"), job(2, "build", at(0), at(2))]);
        expect(stages.map((stage) => stage.map((group) => group.jobs[0]?.label))).toEqual([["build"], ["odd"]]);
    });

    it("keeps jobs that started together in the order they were listed", () => {
        const stages = stagesOf([job(1, "second", at(0), at(1)), job(2, "first", at(0), at(1))]);
        expect(stages.map((stage) => stage.map((group) => group.jobs[0]?.label))).toEqual([["second", "first"]]);
    });

    it("does not guess what waited on a finished job whose end is unknown", () => {
        const stages = stagesOf([job(1, "build", at(0), null), job(2, "deploy", at(5), at(6))]);
        expect(stages).toHaveLength(1);
    });
});

describe("graphOf", () => {
    const release = `jobs:
  approve:
    name: Approve the stable release
    if: \${{ !contains(github.ref_name, '-nightly.') }}
  pushed-checks:
    name: Checks already run on this commit
  checks:
    name: Checks
    needs: pushed-checks # only when the push's own run did not pass
    uses: ./.github/workflows/ci.yml
  build:
    name: Build and verify
    needs: approve
  build-linux:
    name: Build Linux packages
    needs: approve
  publish:
    name: Publish
    needs: [build, build-linux, pushed-checks, checks]
`;
    const ci = `jobs:
  changes:
    name: What changed
  frontend:
    name: Frontend quality, build, and tests
  rust:
    name: Rust formatting, lint, and tests
    needs: changes
  linux:
    name: Linux compile check and core tests
    needs:
      - changes
`;
    const files = { ".github/workflows/release.yml": readJobs(release), ".github/workflows/ci.yml": readJobs(ci) };
    const shape = (graph: Graph): unknown =>
        graph.columns.map((column) => column.map((box) => (box.jobs.length > 1 ? box.jobs.map((each) => each.label) : box.jobs[0]?.label)));

    it("lays out the release run of v0.5.3-nightly.6 as GitHub drew it", () => {
        const jobs = [
            job(1, "Checks already run on this commit", at(0), at(1)),
            job(2, "Build and verify", at(2), at(30)),
            job(3, "Build Linux packages", at(2), at(20)),
            { ...job(4, "Approve the stable release", at(0), at(0)), conclusion: "skipped" },
            { ...job(5, "Checks", at(1), at(1)), conclusion: "skipped" },
            job(6, "Publish", at(31), at(32)),
        ];
        const graph = graphOf(jobs, files, ".github/workflows/release.yml");
        expect(shape(graph)).toEqual([
            ["Checks already run on this commit", "Approve the stable release"],
            ["Checks / What changed", "Checks / Frontend quality, build, and tests", ["Build and verify", "Build Linux packages"]],
            [["Checks / Linux compile check and core tests", "Checks / Rust formatting, lint, and tests"]],
            ["Publish"],
        ]);
        expect(graph.columns[2]?.[0]?.jobs.every((each) => each.job === null && each.skipped)).toBe(true);
        expect(graph.edges.map((edge) => edge.join(" > ")).sort()).toEqual(
            [
                "approve > build",
                "build > publish",
                "checks/changes > checks/rust",
                "checks/frontend > publish",
                "checks/rust > publish",
                "pushed-checks > checks/changes",
                "pushed-checks > checks/frontend",
                "pushed-checks > publish",
            ].sort(),
        );
    });

    it("keeps a matrix job's runs in one box under its name", () => {
        const matrix = { "m.yml": readJobs("jobs:\n  host:\n    name: Build the core (\${{ matrix.arch }})\n") };
        const graph = graphOf([job(1, "Build the core (x86_64)", at(0), at(1)), job(2, "Build the core (aarch64)", at(0), at(1))], matrix, "m.yml");
        expect(graph.columns[0]?.[0]?.title).toBe("Build the core");
        expect(graph.columns[0]?.[0]?.jobs.map((each) => each.label)).toEqual(["x86_64", "aarch64"]);
    });

    it("reads the stages off the clock when a job is not in the file", () => {
        const graph = graphOf([job(1, "Something else", at(0), at(1))], files, ".github/workflows/release.yml");
        expect(shape(graph)).toEqual([["Something else"]]);
    });
});

describe("stepStarts", () => {
    const line = (number: number, second: number): LogLine => ({
        number,
        timestamp: `2026-09-27T10:00:${String(second).padStart(2, "0")}.5000000Z`,
        text: `line ${number}`,
    });
    const step = (number: number, second: number | null): Step => ({
        number,
        name: `step ${number}`,
        status: "completed",
        conclusion: "success",
        startedAt: second === null ? null : `2026-09-27T10:00:${String(second).padStart(2, "0")}Z`,
        completedAt: null,
    });

    it("finds the first line each step wrote", () => {
        const lines = [line(1, 0), line(2, 1), line(3, 4), line(4, 4), line(5, 9)];
        const starts = stepStarts(lines, [step(1, 0), step(2, 4), step(3, 9)]);
        expect([...starts]).toEqual([
            [1, 0],
            [2, 2],
            [3, 4],
        ]);
    });

    it("steps over lines with no time on them", () => {
        const lines = [{ number: 1, timestamp: null, text: "no clock" }, line(2, 4)];
        expect([...stepStarts(lines, [step(1, 4)])]).toEqual([[1, 1]]);
    });

    it("skips a step that never ran and one that wrote nothing", () => {
        const starts = stepStarts([line(1, 0)], [step(1, null), step(2, 0), step(3, 30)]);
        expect([...starts]).toEqual([[2, 0]]);
    });
});
