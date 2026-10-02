import { useEffect, useMemo, useRef, useState } from "react";
import { useResourceEnabled } from "../../plugin-api/resources";
import { copyText, notify, swallow } from "../../plugin-api/host";
import { EmptyState, SkeletonRows, VirtualLogList } from "../../plugin-api/ui";
import { failureMessage, type Job, type LogLine, type RepoRef } from "../api";
import { stepStarts } from "../jobGraph";
import { jobLogR } from "../resources";
import { useEvery } from "./hooks";

/** Runner logs mark their sections with `##[...]`, which is noise on screen. */
const MARKUP = /^##\[(?:group|endgroup|section|command)\]/u;
const ENDGROUP = /^##\[endgroup\]/u;

function clean(text: string): string {
    return text.replace(MARKUP, "");
}

const LIVE_REFRESH_MS = 5_000;

function findMatches(lines: readonly LogLine[], needle: string): number[] {
    if (!needle) return [];
    const found: number[] = [];
    lines.forEach((line, index) => {
        if (line.text.toLowerCase().includes(needle)) found.push(index);
    });
    return found;
}

const needleOf = (query: string) => query.trim().toLowerCase();

interface Props {
    repo: RepoRef;
    job: Job;
    active: boolean;
    step: { number: number } | null;
}

export function JobLogView({ repo, job, active, step }: Props) {
    const running = job.status !== "completed";
    const log = useResourceEnabled(active, jobLogR, repo, job.id);
    const [query, setQuery] = useState("");
    const [match, setMatch] = useState(0);
    const [jump, setJump] = useState<{ index: number } | null>(null);
    const [stepLine, setStepLine] = useState<number | null>(null);

    useEvery(active && running, LIVE_REFRESH_MS, () => void log.refresh());

    // The last lines a job writes, usually the error, land after the last
    // read made while it was running, so a finished job is read once more.
    const refreshLog = useRef(log.refresh);
    refreshLog.current = log.refresh;
    const owesFinalRead = useRef(running);
    useEffect(() => {
        if (running) {
            owesFinalRead.current = true;
            return;
        }
        if (!owesFinalRead.current || !active) return;
        owesFinalRead.current = false;
        void refreshLog.current();
    }, [running, active]);

    const lines = useMemo(() => log.data?.lines ?? [], [log.data]);
    const starts = useMemo(() => stepStarts(lines, job.steps), [lines, job.steps]);
    const needle = needleOf(query);
    const matches = useMemo(() => findMatches(lines, needle), [lines, needle]);
    const matched = useMemo(() => new Set(matches), [matches]);

    const shownStep = useRef<{ number: number } | null>(null);
    useEffect(() => {
        if (!step || step === shownStep.current) return;
        const index = starts.get(step.number);
        if (index === undefined) return;
        shownStep.current = step;
        setStepLine(index);
        setJump({ index });
    }, [step, starts]);

    const goTo = (next: number) => {
        if (matches.length === 0) return;
        const wrapped = (next + matches.length) % matches.length;
        setMatch(wrapped);
        setJump({ index: matches[wrapped] ?? 0 });
    };

    if (log.status === "loading" && !log.data) return <SkeletonRows rows={12} label="Loading log" />;
    if (log.error) {
        return (
            <EmptyState
                title="Could not read the log"
                message={failureMessage(log.error)}
                tone="error"
                action={{ label: "Try again", onClick: () => void log.refresh() }}
            />
        );
    }
    if (log.data?.expired) {
        return <EmptyState title="The log is gone" message="GitHub keeps job logs for a limited time, and this one has aged out." />;
    }
    if (lines.length === 0) {
        return <EmptyState message={running ? "This job has not written anything yet." : "This job wrote no log."} />;
    }

    const copyAll = () =>
        void copyText(lines.map((line) => clean(line.text)).join("\n"))
            .then(() => notify("success", `Copied ${lines.length} lines`))
            .catch(swallow("copy the log"));

    const current = matches[match] ?? -1;
    return (
        <div className="gha-log">
            <div className="gha-log-head">
                <input
                    className="gha-log-search"
                    type="search"
                    placeholder="Search the log"
                    value={query}
                    spellCheck={false}
                    onChange={(event) => {
                        const first = findMatches(lines, needleOf(event.target.value))[0];
                        setQuery(event.target.value);
                        setMatch(0);
                        if (first !== undefined) setJump({ index: first });
                    }}
                    onKeyDown={(event) => {
                        if (event.key !== "Enter") return;
                        event.preventDefault();
                        goTo(event.shiftKey ? match - 1 : match + 1);
                    }}
                />
                {needle && <span className="gha-dim gha-mono">{matches.length === 0 ? "no matches" : `${match + 1} of ${matches.length}`}</span>}
                {matches.length > 1 && (
                    <>
                        <button type="button" className="gha-link" onClick={() => goTo(match - 1)}>
                            Previous
                        </button>
                        <button type="button" className="gha-link" onClick={() => goTo(match + 1)}>
                            Next
                        </button>
                    </>
                )}
                <span className="gha-log-spacer" />
                <span className="gha-dim">
                    {lines.length} line{lines.length === 1 ? "" : "s"}
                    {running && " so far"}
                </span>
                <button type="button" className="gha-link" onClick={copyAll}>
                    Copy
                </button>
                <button type="button" className="gha-link" onClick={() => void log.refresh()}>
                    Refresh
                </button>
            </div>
            <VirtualLogList
                items={lines}
                className="gha-log-scroll"
                follow={running && !jump && !needle}
                jumpTo={jump}
                getItemKey={(line) => line.number}
                rowClassName={(line, index) => {
                    const group = MARKUP.test(line.text) && !ENDGROUP.test(line.text) ? " group" : "";
                    const hit = index === current ? " hit current" : matched.has(index) ? " hit" : "";
                    const anchor = index === stepLine ? " anchor" : "";
                    return `gha-log-line${group}${hit}${anchor}`;
                }}
                renderRow={(line) => (
                    <>
                        <span className="gha-log-number gha-mono">{line.number}</span>
                        <span className="gha-log-text gha-mono">{clean(line.text) || " "}</span>
                    </>
                )}
            />
        </div>
    );
}
