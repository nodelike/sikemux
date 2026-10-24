import { memo, useId, useLayoutEffect, useMemo, useState } from "react";
import type { GraphBox, GraphEdge, GraphJob, Job, OpenRun, Run } from "../api";
import { elapsedMs, outcomeOf } from "../runStatus";
import { StatusMark } from "./ActionsIcon";
import { coarse } from "./hooks";
import { formatDuration } from "../../lib/format";
import { basename } from "../../lib/paths";

interface Props {
    run: Run;
    /** The run as the core laid out its jobs. */
    read: OpenRun;
    now: number;
    openJob: string | null;
    onOpen: (jobId: string) => void;
    fileShown: boolean;
    /** Left out when the host cannot show the file a run came from. */
    onToggleFile: (() => void) | null;
}

interface Shared {
    jobs: ReadonlyMap<string, Job>;
    now: number;
    openJob: string | null;
    onOpen: (jobId: string) => void;
}

function JobRow({ entry, shared }: { entry: GraphJob; shared: Shared }) {
    const { label } = entry;
    const job = entry.job === null ? undefined : shared.jobs.get(entry.job);
    if (!job)
        return (
            <div className="gha-graph-job" data-idle="1" title={label}>
                <StatusMark outcome={entry.skipped ? "skipped" : "idle"} />
                <span className="gha-graph-job-name">{label}</span>
            </div>
        );
    const outcome = outcomeOf(job);
    const took = elapsedMs(job.startedAt, job.completedAt, job.completedAt ? coarse(shared.now) : shared.now);
    return (
        <button
            type="button"
            className="gha-graph-job"
            data-on={shared.openJob === job.id ? "1" : "0"}
            title={job.name}
            onClick={() => shared.onOpen(job.id)}>
            <StatusMark outcome={outcome} />
            <span className="gha-graph-job-name">{label}</span>
            {took !== null && outcome !== "skipped" && <span className="gha-graph-job-took">{formatDuration(took)}</span>}
        </button>
    );
}

function Box({ box, shared }: { box: GraphBox; shared: Shared }) {
    return (
        <div className="gha-graph-box" data-node={box.id} data-many={box.title || box.jobs.length > 1 ? "1" : "0"}>
            {box.title && (
                <div className="gha-graph-group-title" title={box.title}>
                    {box.title}
                </div>
            )}
            {box.jobs.map((entry) => (
                <JobRow key={entry.job ?? entry.label} entry={entry} shared={shared} />
            ))}
        </div>
    );
}

interface Drawn {
    width: number;
    height: number;
    lines: string[];
    dots: [number, number][];
    /** Where the boxes are, so a line passing behind one never shows through it. */
    holes: { x: number; y: number; width: number; height: number }[];
}

function offsetWithin(element: HTMLElement, root: HTMLElement): { x: number; y: number } {
    let x = 0;
    let y = 0;
    let at: HTMLElement | null = element;
    while (at && at !== root) {
        x += at.offsetLeft;
        y += at.offsetTop;
        at = at.offsetParent as HTMLElement | null;
    }
    return { x, y };
}

const ELBOW = 6;

/** Out of a box sideways, along the middle of the gap before the box it enters, and in sideways, turning on round corners. */
function route(x1: number, y1: number, x2: number, y2: number, turn: number): string {
    if (Math.abs(y2 - y1) < 0.5) return `M${x1} ${y1}H${x2}`;
    const radius = Math.max(0, Math.min(ELBOW, Math.abs(y2 - y1) / 2, turn - x1, x2 - turn));
    const down = y2 > y1 ? 1 : -1;
    return (
        `M${x1} ${y1}H${turn - radius}Q${turn} ${y1} ${turn} ${y1 + down * radius}` +
        `V${y2 - down * radius}Q${turn} ${y2} ${turn + radius} ${y2}H${x2}`
    );
}

/** Lines meet a box at the middle of its first row, so a tall box joins where its first job is. */
function measure(canvas: HTMLElement, edges: readonly GraphEdge[]): Drawn {
    const columns = [...canvas.querySelectorAll<HTMLElement>("[data-column]")].map((column) => {
        const { x } = offsetWithin(column, canvas);
        return { left: x, right: x + column.offsetWidth };
    });
    const boxes = new Map<string, { left: number; right: number; y: number; column: number }>();
    const holes: Drawn["holes"] = [];
    for (const box of canvas.querySelectorAll<HTMLElement>("[data-node]")) {
        const row = box.querySelector<HTMLElement>(".gha-graph-job") ?? box;
        const at = offsetWithin(box, canvas);
        const middle = offsetWithin(row, canvas).y + row.offsetHeight / 2;
        const column = Number(box.closest<HTMLElement>("[data-column]")?.dataset.column ?? 0);
        if (box.dataset.node) boxes.set(box.dataset.node, { left: at.x, right: at.x + box.offsetWidth, y: middle, column });
        holes.push({ x: at.x, y: at.y, width: box.offsetWidth, height: box.offsetHeight });
    }
    const lines: string[] = [];
    const dots = new Map<string, [number, number]>();
    for (const { from, to } of edges) {
        const start = boxes.get(from);
        const end = boxes.get(to);
        if (!start || !end) continue;
        const before = columns[end.column - 1]?.right ?? start.right;
        lines.push(route(start.right, start.y, end.left, end.y, (before + end.left) / 2));
        dots.set(`${start.right},${start.y}`, [start.right, start.y]);
        dots.set(`${end.left},${end.y}`, [end.left, end.y]);
    }
    return { width: canvas.offsetWidth, height: canvas.offsetHeight, lines, dots: [...dots.values()], holes };
}

function Edges({ canvas, edges }: { canvas: HTMLDivElement | null; edges: readonly GraphEdge[] }) {
    const [drawn, setDrawn] = useState<Drawn | null>(null);
    const mask = `gha-graph-edges-${useId().replace(/[^\w-]/g, "")}`;
    useLayoutEffect(() => {
        const root = canvas;
        if (!root) return;
        let last = "";
        const draw = () => {
            const next = measure(root, edges);
            const key = `${next.width}x${next.height}:${next.lines.join("")}:${next.holes.map((hole) => `${hole.x},${hole.y},${hole.height}`).join(" ")}`;
            if (key === last) return;
            last = key;
            setDrawn(next);
        };
        draw();
        const observer = new ResizeObserver(draw);
        observer.observe(root);
        for (const box of root.querySelectorAll("[data-node]")) observer.observe(box);
        return () => observer.disconnect();
    }, [canvas, edges]);
    if (!drawn) return null;
    return (
        <svg className="gha-graph-edges" width={drawn.width} height={drawn.height} aria-hidden="true">
            <mask id={mask} maskUnits="userSpaceOnUse" x={0} y={0} width={drawn.width} height={drawn.height}>
                <rect width={drawn.width} height={drawn.height} fill="white" />
                {drawn.holes.map((hole) => (
                    <rect key={`${hole.x},${hole.y}`} x={hole.x} y={hole.y} width={hole.width} height={hole.height} fill="black" />
                ))}
            </mask>
            <g mask={`url(#${mask})`}>
                {drawn.lines.map((line) => (
                    <path key={line} d={line} />
                ))}
            </g>
            {drawn.dots.map(([x, y]) => (
                <circle key={`${x},${y}`} cx={x} cy={y} r={3} />
            ))}
        </svg>
    );
}

export const JobGraph = memo(function JobGraph({ run, read, now, openJob, onOpen, fileShown, onToggleFile }: Props) {
    const { graph } = read;
    const jobs = useMemo(() => new Map(read.jobs.map(({ job }) => [job.id, job])), [read.jobs]);
    const [canvas, setCanvas] = useState<HTMLDivElement | null>(null);
    const shared = useMemo(() => ({ jobs, now, openJob, onOpen }), [jobs, now, openJob, onOpen]);
    const name = run.path ? basename(run.path) : null;
    const total = read.jobs.length;

    return (
        <section className="gha-graph">
            <div className="gha-graph-head">
                <span className="gha-graph-file">{name ?? run.name}</span>
                {total > 0 && (
                    <span className="gha-dim">
                        {read.done} of {total} job{total === 1 ? "" : "s"} done
                        {read.failed > 0 && <span className="gha-failed-count"> · {read.failed} failed</span>}
                    </span>
                )}
                <span className="gha-graph-spacer" />
                {onToggleFile && run.path && !run.path.startsWith("dynamic/") && (
                    <button type="button" className="gha-link" onClick={onToggleFile}>
                        {fileShown ? "Hide workflow file" : "Workflow file"}
                    </button>
                )}
            </div>
            {graph.columns.length === 0 ? (
                <div className="gha-dim gha-graph-empty">No jobs yet</div>
            ) : (
                <div className="gha-graph-stages">
                    <div className="gha-graph-canvas" ref={setCanvas}>
                        <Edges canvas={canvas} edges={graph.edges} />
                        <div className="gha-graph-columns">
                            {graph.columns.map((column, index) => (
                                <div className="gha-graph-stage" data-column={index} key={column.map((box) => box.id).join("|")}>
                                    {column.map((box) => (
                                        <Box key={box.id} box={box} shared={shared} />
                                    ))}
                                </div>
                            ))}
                        </div>
                    </div>
                </div>
            )}
        </section>
    );
});
