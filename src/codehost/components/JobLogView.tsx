import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useResourceEnabled } from "../../plugin-api/resources";
import { copyText, notify, swallow } from "../../plugin-api/host";
import {
    EmptyState,
    IconArrowDown,
    IconArrowUp,
    IconCollapseAll,
    IconCopy,
    IconExpandAll,
    IconRefresh,
    IconSearch,
    SkeletonRows,
} from "../../plugin-api/ui";
import { CARD_HEADER_HEIGHT, CardStream, type CardStreamHandle } from "../../ui/CardStream";
import { type Job, type JobLogLine, type LogColor, type LogSpan, type RepoRef, type Step } from "../api";
import { errorText } from "../../lib/format";
import { jobLogViewR } from "../lists";
import { elapsedMs, outcomeOf } from "../runStatus";
import { formatDuration } from "../../lib/format";
import { StatusMark } from "./ActionsIcon";
import { ansiCss } from "../../lib/ansi";
import { CodeRun, fenceLanguage, useCodeTokens } from "../../chat/codeHighlight";

const SHELL = fenceLanguage("bash");

const LINE_HEIGHT = 18;
const BODY_PADDING = 4;
/** How close to the bottom the reader must be for a running job's new lines to keep the view at the bottom. */
const FOLLOW_SLACK = 48;

function findMatches(plain: readonly string[], needle: string): number[] {
    if (!needle) return [];
    const found: number[] = [];
    plain.forEach((text, index) => {
        if (text.toLowerCase().includes(needle)) found.push(index);
    });
    return found;
}

/** The scripts the step's `Run` lines repeat, as one text to colour as bash, and which line of it each log line is. */
function commandsOf(lines: readonly JobLogLine[], from: number, to: number): { text: string; at: Map<number, number> } {
    const at = new Map<number, number>();
    const script: string[] = [];
    let inScript = false;
    for (let index = from; index < to; index += 1) {
        const line = lines[index];
        if (!line?.script) {
            inScript = false;
            continue;
        }
        if (!inScript && script.length > 0) script.push("");
        inScript = true;
        at.set(index, script.length);
        script.push(line.text);
    }
    return { text: script.join("\n"), at };
}

const css = (color: LogColor | undefined) => (color ? ansiCss(color.kind === "palette" ? color.index : color.hex) : undefined);

function spanStyle(span: LogSpan): CSSProperties {
    return {
        color: css(span.fg),
        background: css(span.bg),
        fontWeight: span.bold ? 600 : undefined,
        opacity: span.dim ? 0.65 : undefined,
        fontStyle: span.italic ? "italic" : undefined,
        textDecoration: span.underline ? "underline" : undefined,
    };
}

/** A line in the colours the job printed it in. */
const LogText = memo(function LogText({ line }: { line: JobLogLine }) {
    if (line.spans.length === 0) return line.text || " ";
    return (
        <>
            {line.spans.map((span, index) => (
                <span key={index} style={spanStyle(span)}>
                    {span.text}
                </span>
            ))}
        </>
    );
});

const needleOf = (query: string) => query.trim().toLowerCase();

/** One step's stretch of the job's log: lines `from` up to `to`. */
interface Section {
    key: string;
    step: Step | null;
    from: number;
    to: number;
}

interface Jump {
    key: string;
    line: number;
}

interface Marks {
    matched: ReadonlySet<number>;
    current: number;
}

interface StepLinesProps {
    lines: readonly JobLogLine[];
    section: Section;
    start: number;
    scroller: () => HTMLElement | null;
    marks: Marks;
    jump: Jump | null;
    onJumped: () => void;
}

/** The list scrolls itself and every open step shares it, so a step's own lines never move it. */
function leaveScrollAlone(): void {}

/** An open step's lines, drawn only where the list is scrolled to, however long the step ran. */
const StepLines = memo(function StepLines({ lines, section, start, scroller, marks, jump, onJumped }: StepLinesProps) {
    const rows = useRef<HTMLDivElement>(null);
    const [margin, setMargin] = useState(start + CARD_HEADER_HEIGHT + BODY_PADDING);
    useLayoutEffect(() => {
        if (rows.current) setMargin(start + rows.current.offsetTop);
    }, [start]);
    const { from, to } = section;
    const virtualizer = useVirtualizer({
        count: to - from,
        getScrollElement: scroller,
        estimateSize: () => LINE_HEIGHT,
        overscan: 24,
        scrollMargin: margin,
        scrollPaddingStart: CARD_HEADER_HEIGHT + LINE_HEIGHT * 2,
        getItemKey: (index) => lines[from + index]?.number ?? index,
        initialRect: { width: 1000, height: 800 },
        initialOffset: () => scroller()?.scrollTop ?? 0,
        scrollToFn: leaveScrollAlone,
    });

    const commands = useMemo(() => commandsOf(lines, from, to), [lines, from, to]);
    const coloured = useCodeTokens(commands.text, commands.text ? SHELL : null);

    const mine = jump?.key === section.key ? jump : null;
    useEffect(() => {
        if (!mine) return;
        const frame = window.requestAnimationFrame(() => {
            const list = scroller();
            const target = virtualizer.getOffsetForIndex(Math.max(0, mine.line - from), "start")?.[0];
            if (list && target !== undefined) list.scrollTop = target;
            onJumped();
        });
        return () => window.cancelAnimationFrame(frame);
    }, [mine, from, virtualizer, scroller, onJumped]);

    if (to === from) return <div className="gha-step-log gha-dim gha-step-empty">This step wrote nothing.</div>;
    return (
        <div className="gha-step-log">
            <div ref={rows} className="gha-step-rows" style={{ height: virtualizer.getTotalSize() }}>
                {virtualizer.getVirtualItems().map((row) => {
                    const index = from + row.index;
                    const line = lines[index];
                    if (!line) return null;
                    const kind = line.mark;
                    const tokens = coloured?.[commands.at.get(index) ?? -1];
                    const hit = index === marks.current ? " hit current" : marks.matched.has(index) ? " hit" : "";
                    return (
                        <div
                            key={row.key}
                            ref={virtualizer.measureElement}
                            data-index={row.index}
                            className={`gha-log-line${kind && kind !== "endgroup" ? ` ${kind}` : ""}${hit}`}
                            style={{ transform: `translateY(${row.start - virtualizer.options.scrollMargin}px)` }}>
                            <span className="gha-log-number gha-mono">{row.index + 1}</span>
                            <span className="gha-log-text gha-mono">{tokens ? <CodeRun tokens={tokens} /> : <LogText line={line} />}</span>
                        </div>
                    );
                })}
            </div>
        </div>
    );
});

interface Props {
    project: string | null;
    repo: RepoRef;
    runId: string;
    job: Job;
    now: number;
    active: boolean;
}

const NO_LINES: JobLogLine[] = [];

export function JobLogView({ project, repo, runId, job, now, active }: Props) {
    const running = job.status !== "completed";
    const log = useResourceEnabled(active, jobLogViewR, project, repo, runId, job.id);
    const [query, setQuery] = useState("");
    const [match, setMatch] = useState(0);
    const [jump, setJump] = useState<Jump | null>(null);
    const [opened, setOpened] = useState<Set<string>>(() => new Set());
    const stream = useRef<CardStreamHandle>(null);
    const following = useRef(true);

    const lines = log.data?.lines ?? NO_LINES;
    const plain = useMemo(() => lines.map((line) => line.text), [lines]);
    const sections = useMemo(
        (): Section[] =>
            (log.data?.sections ?? []).map(({ step, from, to }) => ({
                key: step === null ? "log" : `step:${step}`,
                step: step === null ? null : (job.steps.find((each) => each.number === step) ?? null),
                from,
                to,
            })),
        [log.data, job.steps],
    );
    const keys = useMemo(() => sections.map((section) => section.key), [sections]);
    const byKey = useMemo(() => new Map(sections.map((section) => [section.key, section])), [sections]);
    const collapsed = useMemo(() => new Set(keys.filter((key) => !opened.has(key))), [keys, opened]);
    const needle = needleOf(query);
    const matches = useMemo(() => findMatches(plain, needle), [plain, needle]);
    const current = matches[match] ?? -1;
    const marks = useMemo(() => ({ matched: new Set(matches), current }), [matches, current]);

    const show = useCallback(
        (line: number) => {
            const at = sections.findIndex((section) => line >= section.from && line < section.to);
            const section = sections[at];
            if (!section) return;
            following.current = false;
            setOpened((was) => (was.has(section.key) ? was : new Set(was).add(section.key)));
            setJump({ key: section.key, line });
            const list = stream.current?.scroller();
            if (list) list.scrollTop = stream.current?.cardStart(at) ?? list.scrollTop;
        },
        [sections],
    );

    // GitHub's way: a step that failed opens, and the first one to fail is brought into view; a running job's newest
    // step opens and is followed. A step that passed stays folded.
    const seen = useRef(new Map<string, string>());
    const broughtFailure = useRef(false);
    useEffect(() => {
        const opening: string[] = [];
        let firstFailed: Section | null = null;
        for (const section of sections) {
            const outcome = section.step ? outcomeOf(section.step) : "log";
            if (seen.current.get(section.key) === outcome) continue;
            seen.current.set(section.key, outcome);
            if (outcome === "failure") {
                opening.push(section.key);
                firstFailed ??= section;
            } else if (outcome === "running" || outcome === "log") {
                opening.push(section.key);
            }
        }
        if (opening.length > 0) setOpened((was) => new Set([...was, ...opening]));
        if (firstFailed && !broughtFailure.current && lines.length > 0) {
            broughtFailure.current = true;
            show(firstFailed.from);
        }
    }, [sections, lines.length, show]);

    useEffect(() => {
        if (!running || !following.current || needle) return;
        const frame = window.requestAnimationFrame(() => {
            const list = stream.current?.scroller();
            if (list) list.scrollTop = list.scrollHeight;
        });
        return () => window.cancelAnimationFrame(frame);
    }, [running, lines.length, opened, needle]);

    const toggle = useCallback((key: string) => {
        setOpened((was) => {
            const next = new Set(was);
            next.has(key) ? next.delete(key) : next.add(key);
            return next;
        });
    }, []);
    const jumped = useCallback(() => setJump(null), []);
    const scroller = useCallback(() => stream.current?.scroller() ?? null, []);

    if (log.status === "loading" && !log.data) return <SkeletonRows rows={12} label="Loading log" />;
    if (log.error) {
        return (
            <EmptyState
                title="Could not read the log"
                message={errorText(log.error)}
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
        void copyText(plain.join("\n"))
            .then(() => notify("success", `Copied ${lines.length} lines`))
            .catch(swallow("copy the log"));

    const goTo = (next: number) => {
        if (matches.length === 0) return;
        const wrapped = (next + matches.length) % matches.length;
        setMatch(wrapped);
        show(matches[wrapped] ?? 0);
    };

    return (
        <div className="gha-log">
            <div className="git-filter gha-log-head">
                <IconSearch size={13} />
                <input
                    type="search"
                    placeholder="Search the log"
                    aria-label="Search the log"
                    value={query}
                    spellCheck={false}
                    onChange={(event) => {
                        const first = findMatches(plain, needleOf(event.target.value))[0];
                        setQuery(event.target.value);
                        setMatch(0);
                        if (first !== undefined) show(first);
                    }}
                    onKeyDown={(event) => {
                        if (event.key !== "Enter") return;
                        event.preventDefault();
                        goTo(event.shiftKey ? match - 1 : match + 1);
                    }}
                />
                {needle && <span className="git-filter-count">{matches.length === 0 ? "no matches" : `${match + 1} of ${matches.length}`}</span>}
                {matches.length > 1 && (
                    <>
                        <button
                            type="button"
                            className="git-foot-btn icon"
                            aria-label="Previous"
                            title="Previous match"
                            onClick={() => goTo(match - 1)}>
                            <IconArrowUp size={13} />
                        </button>
                        <button type="button" className="git-foot-btn icon" aria-label="Next" title="Next match" onClick={() => goTo(match + 1)}>
                            <IconArrowDown size={13} />
                        </button>
                    </>
                )}
                <span className="gha-log-count">
                    {lines.length} line{lines.length === 1 ? "" : "s"}
                    {running && " so far"}
                </span>
                {collapsed.size === 0 ? (
                    <button type="button" className="git-foot-btn" onClick={() => setOpened(new Set())}>
                        <IconCollapseAll size={13} />
                        Collapse all
                    </button>
                ) : (
                    <button type="button" className="git-foot-btn" onClick={() => setOpened(new Set(keys))}>
                        <IconExpandAll size={13} />
                        Expand all
                    </button>
                )}
                <button type="button" className="git-foot-btn" onClick={copyAll}>
                    <IconCopy size={13} />
                    Copy
                </button>
                <button type="button" className="git-foot-btn" onClick={() => void log.refresh()}>
                    <IconRefresh size={13} />
                    Refresh
                </button>
            </div>
            <div className="merge-review gha-log-steps">
                <CardStream
                    handle={stream}
                    keys={keys}
                    collapsed={collapsed}
                    onToggle={toggle}
                    estimate={(key) => {
                        const section = byKey.get(key);
                        return CARD_HEADER_HEIGHT + BODY_PADDING * 2 + Math.max(1, (section?.to ?? 0) - (section?.from ?? 0)) * LINE_HEIGHT;
                    }}
                    toggleLabel={(key, open) => `${open ? "Collapse" : "Expand"} ${byKey.get(key)?.step?.name ?? "the log"}`}
                    onScroll={(list) => {
                        following.current = list.scrollHeight - list.scrollTop - list.clientHeight < FOLLOW_SLACK;
                    }}
                    header={(key) => {
                        const step = byKey.get(key)?.step ?? null;
                        if (!step) return <span className="acc-name gha-step-title">{job.name}</span>;
                        const outcome = outcomeOf(step);
                        const took = elapsedMs(step.startedAt, step.completedAt, now);
                        return (
                            <>
                                <StatusMark outcome={outcome} size={14} />
                                <span className="acc-name gha-step-title" data-outcome={outcome}>
                                    {step.name}
                                </span>
                                <span className="acc-grow" />
                                {took !== null && outcome !== "skipped" && (
                                    <span className="merge-file-status gha-step-took">{formatDuration(took)}</span>
                                )}
                            </>
                        );
                    }}
                    body={(key, start) => {
                        const section = byKey.get(key);
                        if (!section) return null;
                        return (
                            <StepLines
                                lines={lines}
                                section={section}
                                start={start}
                                scroller={scroller}
                                marks={marks}
                                jump={jump}
                                onJumped={jumped}
                            />
                        );
                    }}
                />
            </div>
        </div>
    );
}
