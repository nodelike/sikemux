import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { subscribeTheme } from "../../themes/bus";
import type { GitCommit } from "../../api/git";
import { EmptyState } from "../Panel";

const ROW_H = 30;
/** Beyond two, refs crowd the subject out of the row entirely. */
const MAX_ROW_REFS = 2;
const LANE_W = 14; // horizontal gap between lanes
const X0 = 14; // x of lane 0's centre
const NODE_R = 4;
const GUTTER_PAD = 18; // breathing room between the deepest lane and the text

const gutterWidth = (maxLanes: number) => X0 + Math.max(0, maxLanes - 1) * LANE_W + GUTTER_PAD;
const laneX = (lane: number) => X0 + lane * LANE_W;

interface Edge {
    lane: number;
    color: number;
    unpushed: boolean;
}
interface RowLayout {
    lane: number;
    colorIdx: number;
    isHead: boolean;
    unpushed: boolean;
    merges: (Edge & { fromLane: number })[]; // top edge → node (a child line landing on this commit)
    through: Edge[]; // straight verticals passing this row
    branches: (Edge & { toLane: number })[]; // node → bottom edge (this commit's parents)
}

export function computeGraph(commits: GitCommit[]): { rows: RowLayout[]; maxLanes: number } {
    const visible = new Set(commits.map((c) => c.full_hash));
    const drawn = new Set<string>(); // a lane waiting for one of these would never close
    const lanes: (string | null)[] = []; // lanes[i] = full hash that lane i is currently waiting for
    const laneUp: boolean[] = [];
    const rows: RowLayout[] = [];
    let maxLanes = 0;

    const pad = (to: number) => {
        while (lanes.length <= to) {
            lanes.push(null);
            laneUp.push(false);
        }
    };
    const firstFree = () => {
        const i = lanes.indexOf(null);
        if (i !== -1) return i;
        lanes.push(null);
        laneUp.push(false);
        return lanes.length - 1;
    };

    for (const c of commits) {
        const cUn = c.unpushed;
        const expecting: number[] = [];
        for (let j = 0; j < lanes.length; j++) if (lanes[j] === c.full_hash) expecting.push(j);

        const commitLane = expecting.length > 0 ? expecting[0] : firstFree();
        pad(commitLane);

        const merges = expecting.map((j) => ({ fromLane: j, lane: j, color: j, unpushed: laneUp[j] }));

        const through: Edge[] = [];
        for (let j = 0; j < lanes.length; j++) {
            if (j === commitLane || lanes[j] === null || lanes[j] === c.full_hash) continue;
            through.push({ lane: j, color: j, unpushed: laneUp[j] });
        }

        for (const j of expecting) {
            lanes[j] = null;
            laneUp[j] = false;
        }
        lanes[commitLane] = null;
        laneUp[commitLane] = false;

        const branches: (Edge & { toLane: number })[] = [];
        const parents = c.parents.filter((p) => visible.has(p) && !drawn.has(p));
        if (parents.length > 0) {
            const p0 = parents[0];
            const existing0 = lanes.indexOf(p0);
            if (existing0 !== -1) {
                branches.push({ toLane: existing0, lane: existing0, color: existing0, unpushed: cUn });
            } else {
                lanes[commitLane] = p0;
                laneUp[commitLane] = cUn;
                branches.push({ toLane: commitLane, lane: commitLane, color: commitLane, unpushed: cUn });
            }
            for (let pi = 1; pi < parents.length; pi++) {
                const p = parents[pi];
                let k = lanes.indexOf(p);
                if (k === -1) {
                    k = firstFree();
                    lanes[k] = p;
                    laneUp[k] = cUn;
                }
                branches.push({ toLane: k, lane: k, color: k, unpushed: cUn });
            }
        }

        rows.push({
            lane: commitLane,
            colorIdx: commitLane,
            isHead: c.refs.includes("HEAD"),
            unpushed: cUn,
            merges,
            through,
            branches,
        });

        drawn.add(c.full_hash);

        while (lanes.length > 0 && lanes[lanes.length - 1] === null) {
            lanes.pop();
            laneUp.pop();
        }
        maxLanes = Math.max(maxLanes, lanes.length, commitLane + 1);
    }

    return { rows, maxLanes: Math.max(1, maxLanes) };
}

function initials(name: string): string {
    const parts = name.trim().split(/\s+/).filter(Boolean);
    if (parts.length === 0) return "?";
    if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
    return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}

function authorColor(key: string): string {
    let h = 0;
    for (let i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) >>> 0;
    return `hsl(${h % 360} 62% 64%)`;
}

function readVar(el: HTMLElement, name: string): string {
    return getComputedStyle(el).getPropertyValue(name).trim();
}
const FALLBACK_PALETTE = ["#a277ff", "#61ffca", "#ff6ac1", "#ffca85", "#7cc5ff", "#ff6767"];
const FALLBACK_WARN = "#ffca85";
const FALLBACK_VOID = "#100e16";

interface GraphColors {
    palette: string[];
    unpushed: string;
    void: string;
}

function readPalette(el: HTMLElement): string[] {
    const themed = ["--acc", "--live", "--cmd", "--warn", "--danger"].map((n) => readVar(el, n)).filter(Boolean);
    const all = [...themed, "#7cc5ff", "#ffd166", "#9b8cff"];
    return all.length ? all : FALLBACK_PALETTE;
}

function sizeCanvas(canvas: HTMLCanvasElement, rows: RowLayout[], maxLanes: number): CanvasRenderingContext2D | null {
    const dpr = window.devicePixelRatio || 1;
    const w = gutterWidth(maxLanes);
    const h = Math.max(1, rows.length * ROW_H);
    const width = Math.round(w * dpr);
    const height = Math.round(h * dpr);
    // Assigning a canvas's size throws its bitmap away even when the size is unchanged.
    if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width;
        canvas.height = height;
        canvas.style.width = `${w}px`;
        canvas.style.height = `${h}px`;
    }

    const ctx = canvas.getContext("2d");
    if (!ctx) return null;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    return ctx;
}

/** The selection ring lives on its own layer so moving it does not repaint the graph. */
function drawSelection(canvas: HTMLCanvasElement, rows: RowLayout[], maxLanes: number, selectedIndex: number) {
    const ctx = sizeCanvas(canvas, rows, maxLanes);
    const row = rows[selectedIndex];
    if (!ctx || !row) return;
    ctx.lineWidth = 1.5;
    ctx.strokeStyle = "rgba(255,255,255,.9)";
    ctx.beginPath();
    ctx.arc(laneX(row.lane), selectedIndex * ROW_H + ROW_H / 2, NODE_R + 3, 0, Math.PI * 2);
    ctx.stroke();
}

function draw(canvas: HTMLCanvasElement, rows: RowLayout[], maxLanes: number, colors: GraphColors) {
    const ctx = sizeCanvas(canvas, rows, maxLanes);
    if (!ctx) return;
    ctx.lineCap = "round";

    const { palette, void: voidColor, unpushed: unpushedColor } = colors;
    const col = (i: number) => palette[i % palette.length];
    const edgeColor = (e: { color: number; unpushed: boolean }) => (e.unpushed ? unpushedColor : col(e.color));
    const cp = ROW_H * 0.42;

    ctx.lineWidth = 1.8;
    rows.forEach((r, i) => {
        const yTop = i * ROW_H;
        const yMid = yTop + ROW_H / 2;
        const yBot = yTop + ROW_H;
        const nodeX = laneX(r.lane);

        for (const t of r.through) {
            ctx.strokeStyle = edgeColor(t);
            ctx.beginPath();
            ctx.moveTo(laneX(t.lane), yTop);
            ctx.lineTo(laneX(t.lane), yBot);
            ctx.stroke();
        }
        for (const m of r.merges) {
            const fx = laneX(m.fromLane);
            ctx.strokeStyle = edgeColor(m);
            ctx.beginPath();
            ctx.moveTo(fx, yTop);
            if (m.fromLane === r.lane) ctx.lineTo(nodeX, yMid);
            else ctx.bezierCurveTo(fx, yTop + cp, nodeX, yMid - cp, nodeX, yMid);
            ctx.stroke();
        }
        for (const b of r.branches) {
            const tx = laneX(b.toLane);
            ctx.strokeStyle = edgeColor(b);
            ctx.beginPath();
            ctx.moveTo(nodeX, yMid);
            if (b.toLane === r.lane) ctx.lineTo(tx, yBot);
            else ctx.bezierCurveTo(nodeX, yMid + cp, tx, yBot - cp, tx, yBot);
            ctx.stroke();
        }
    });

    rows.forEach((r, i) => {
        const yMid = i * ROW_H + ROW_H / 2;
        const nodeX = laneX(r.lane);
        const c = r.unpushed ? unpushedColor : col(r.colorIdx);

        ctx.fillStyle = voidColor;
        ctx.beginPath();
        ctx.arc(nodeX, yMid, NODE_R + 1.6, 0, Math.PI * 2);
        ctx.fill();

        if (r.isHead) {
            ctx.lineWidth = 2.2;
            ctx.strokeStyle = c;
            ctx.beginPath();
            ctx.arc(nodeX, yMid, NODE_R + 1, 0, Math.PI * 2);
            ctx.stroke();
            ctx.fillStyle = c;
            ctx.beginPath();
            ctx.arc(nodeX, yMid, 2.3, 0, Math.PI * 2);
            ctx.fill();
        } else {
            ctx.fillStyle = c;
            ctx.beginPath();
            ctx.arc(nodeX, yMid, NODE_R, 0, Math.PI * 2);
            ctx.fill();
        }
    });
}

function RefBadge({ label }: { label: string }) {
    let kind = "branch";
    let text = label;
    if (label === "HEAD" || label.startsWith("HEAD -> ")) {
        kind = "head";
        text = label.startsWith("HEAD -> ") ? label.slice(8) : label;
    } else if (label.startsWith("tag: ")) {
        kind = "tag";
        text = label.slice(5);
    } else if (label.includes("/")) kind = "remote";
    return <span className={`gg-ref ${kind}`}>{text}</span>;
}

export const GitGraph = memo(function GitGraph({
    commits,
    selectedIndex,
    focused,
    range,
    onSelect,
    onActivate,
}: {
    commits: GitCommit[];
    selectedIndex: number;
    focused: boolean;
    range: [number, number] | null;
    onSelect: (i: number) => void;
    onActivate: () => void;
}) {
    const canvasRef = useRef<HTMLCanvasElement>(null);
    const selectionCanvasRef = useRef<HTMLCanvasElement>(null);
    const selRef = useRef<HTMLDivElement>(null);
    const wrapRef = useRef<HTMLDivElement>(null);
    const { rows, maxLanes } = useMemo(() => computeGraph(commits), [commits]);
    const gutter = gutterWidth(maxLanes);
    const [themeRevision, setThemeRevision] = useState(0);
    // Reading a CSS variable forces a style resolve, so the palette is read
    // once per theme rather than on every redraw. Until it is read nothing is drawn.
    const [colors, setColors] = useState<GraphColors | null>(null);

    useEffect(() => subscribeTheme(() => setThemeRevision((n) => n + 1)), []);

    useLayoutEffect(() => {
        const el = wrapRef.current;
        if (!el) return;
        setColors({
            palette: readPalette(el),
            unpushed: readVar(el, "--warn") || FALLBACK_WARN,
            void: readVar(el, "--void") || FALLBACK_VOID,
        });
    }, [themeRevision]);

    useLayoutEffect(() => {
        const canvas = canvasRef.current;
        if (canvas && colors) draw(canvas, rows, maxLanes, colors);
    }, [rows, maxLanes, colors]);

    useLayoutEffect(() => {
        const canvas = selectionCanvasRef.current;
        if (canvas) drawSelection(canvas, rows, maxLanes, selectedIndex);
    }, [rows, maxLanes, selectedIndex]);

    useLayoutEffect(() => {
        if (focused) selRef.current?.scrollIntoView({ block: "nearest" });
    }, [selectedIndex, focused]);

    if (commits.length === 0) return <EmptyState message="no commits" />;

    return (
        <div className="git-graph" style={{ position: "relative" }} ref={wrapRef}>
            <canvas ref={canvasRef} className="git-graph-canvas" aria-hidden />
            <canvas ref={selectionCanvasRef} className="git-graph-canvas" aria-hidden />
            {commits.map((c, i) => {
                const sel = focused && selectedIndex === i;
                const inRange = range !== null && i >= range[0] && i <= range[1];
                const row = rows[i];
                const palette = colors?.palette ?? FALLBACK_PALETTE;
                const hashColor = row?.unpushed ? (colors?.unpushed ?? FALLBACK_WARN) : palette[(row?.colorIdx ?? 0) % palette.length];
                return (
                    <div
                        key={c.full_hash || c.hash}
                        ref={sel ? selRef : undefined}
                        role="button"
                        tabIndex={selectedIndex === i ? 0 : -1}
                        aria-label={`${c.hash} ${c.subject}`}
                        onKeyDown={(event) => {
                            if (event.key === "Enter" || event.key === " ") {
                                event.preventDefault();
                                event.stopPropagation();
                                onSelect(i);
                                onActivate();
                            }
                            if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
                                event.preventDefault();
                                event.stopPropagation();
                                const next =
                                    event.key === "Home"
                                        ? 0
                                        : event.key === "End"
                                          ? commits.length - 1
                                          : Math.max(0, Math.min(commits.length - 1, i + (event.key === "ArrowDown" ? 1 : -1)));
                                onSelect(next);
                                wrapRef.current?.querySelectorAll<HTMLElement>(".gg-row")[next]?.focus();
                            }
                        }}
                        className={`gg-row${sel ? " sel" : ""}${inRange ? " ranged" : ""}`}
                        style={{ height: ROW_H, paddingLeft: gutter }}
                        onClick={() => onSelect(i)}
                        onDoubleClick={onActivate}
                        title={`${c.subject} — ${c.hash} · ${c.author}${c.refs.length ? ` · ${c.refs.join(", ")}` : ""}`}>
                        <span className="gg-hash" style={{ color: hashColor }}>
                            {c.hash}
                        </span>
                        {c.refs.length > 0 && (
                            <span className="gg-refs">
                                {c.refs.slice(0, MAX_ROW_REFS).map((r) => (
                                    <RefBadge key={r} label={r} />
                                ))}
                                {c.refs.length > MAX_ROW_REFS && (
                                    <span className="gg-ref more" title={c.refs.slice(MAX_ROW_REFS).join(", ")}>
                                        +{c.refs.length - MAX_ROW_REFS}
                                    </span>
                                )}
                            </span>
                        )}
                        <span className="gg-subj">{c.subject}</span>
                        <span className="gg-author" style={{ color: authorColor(c.author_email || c.author) }}>
                            {initials(c.author)}
                        </span>
                        <span className="gg-when">{c.date}</span>
                    </div>
                );
            })}
        </div>
    );
});
