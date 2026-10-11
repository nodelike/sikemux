import { useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { notify } from "../../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../../plugin-api/resources";
import { ContextMenu, EmptyState, IconRefresh, SkeletonRows, type ContextMenuItem } from "../../../plugin-api/ui";
import { failureMessage, jiraApi, type JiraBoardView, type JiraColumn, type JiraIssueSummary } from "../api";
import { jiraBoardR } from "../resources";

/** Where each card sits after the moves still on their way to Jira, by issue key and column index. */
export function withMoves(board: JiraBoardView, moves: Record<string, number>): JiraColumn[] {
    const moving = new Set(Object.keys(moves));
    return board.columns.map((column, index) => ({
        ...column,
        issues: [
            ...column.issues.filter((issue) => !moving.has(issue.key)),
            ...board.columns.flatMap((other) => other.issues.filter((issue) => moves[issue.key] === index)),
        ],
    }));
}

/** How long is left in a sprint, in the words a standup uses. */
export function sprintLeft(end: string | null, now = Date.now()): string | null {
    if (!end) return null;
    const left = Date.parse(end) - now;
    if (Number.isNaN(left)) return null;
    if (left < 0) return "ended";
    const days = Math.floor(left / 86_400_000);
    if (days === 0) return "ends today";
    return days === 1 ? "1 day left" : `${days} days left`;
}

const DRAG_START_PX = 5;

export function JiraBoard({
    active,
    boardId,
    site,
    selected,
    onSelect,
}: {
    active: boolean;
    boardId: number;
    site: string;
    selected: string | null;
    onSelect: (key: string) => void;
}) {
    const found = useResourceEnabled(active, jiraBoardR, boardId, site);
    const [moves, setMoves] = useState<Record<string, number>>({});
    const [dragging, setDragging] = useState<{ key: string; over: number | null } | null>(null);
    const [menu, setMenu] = useState<{ x: number; y: number; items: ContextMenuItem[] } | null>(null);
    const justDragged = useRef(false);

    if (found.status === "error") return <EmptyState message={found.error ?? "Jira could not open this board."} tone="error" />;
    if (!found.data) return <SkeletonRows rows={6} label="Loading the board" />;
    const board = found.data;
    const columns = withMoves(board, moves);

    const move = async (issue: JiraIssueSummary, to: number) => {
        const from = columns.findIndex((column) => column.issues.some((each) => each.key === issue.key));
        const target = board.columns[to];
        if (from === to || !target) return;
        setMoves((now) => ({ ...now, [issue.key]: to }));
        try {
            await jiraApi.moveIssue(issue.key, target, site || undefined);
            await found.refresh();
            invalidate((kind) => kind === "jira.search" || kind === "jira.issue");
        } catch (error) {
            notify("error", `Move ${issue.key}: ${failureMessage(error)}`);
        } finally {
            setMoves((now) => {
                const next = { ...now };
                delete next[issue.key];
                return next;
            });
        }
    };

    const columnAt = (x: number, y: number): number | null => {
        const element = document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-column]");
        return element ? Number(element.dataset.column) : null;
    };

    const startDrag = (event: ReactPointerEvent, issue: JiraIssueSummary) => {
        if (event.button !== 0) return;
        const start = { x: event.clientX, y: event.clientY };
        let moved = false;
        const onMove = (next: PointerEvent) => {
            if (!moved && Math.hypot(next.clientX - start.x, next.clientY - start.y) < DRAG_START_PX) return;
            moved = true;
            setDragging({ key: issue.key, over: columnAt(next.clientX, next.clientY) });
        };
        const onUp = (next: PointerEvent) => {
            window.removeEventListener("pointermove", onMove);
            window.removeEventListener("pointerup", onUp);
            setDragging(null);
            if (!moved) return;
            justDragged.current = true;
            setTimeout(() => (justDragged.current = false), 0);
            const to = columnAt(next.clientX, next.clientY);
            if (to !== null) void move(issue, to);
        };
        window.addEventListener("pointermove", onMove);
        window.addEventListener("pointerup", onUp);
    };

    const left = sprintLeft(board.sprint?.end ?? null);

    return (
        <section className="jira-board" aria-label={board.name}>
            <header className="jira-board-head">
                <div className="jira-board-title">
                    <h2>{board.name}</h2>
                    <span className="jira-chip static">{board.kind}</span>
                    {board.sprint && (
                        <span className="jira-meta">
                            {board.sprint.name}
                            {left && ` · ${left}`}
                        </span>
                    )}
                    <span className="jira-grow" />
                    <button
                        type="button"
                        className="jira-icon-button"
                        title="Refresh"
                        aria-label="Refresh the board"
                        onClick={() => void found.refresh()}>
                        <IconRefresh size={13} />
                    </button>
                </div>
                {board.sprint?.goal && <p className="jira-board-goal">{board.sprint.goal}</p>}
                {board.truncated && <p className="jira-meta">Showing the first 300 issues. Narrow the board in Jira to see the rest.</p>}
            </header>
            {board.kind === "scrum" && !board.sprint ? (
                <EmptyState message="No sprint is running on this board." />
            ) : (
                <div className="jira-columns">
                    {columns.map((column, index) => (
                        <div
                            key={column.name}
                            className={`jira-column${dragging?.over === index ? " drop" : ""}`}
                            data-column={index}
                            role="list"
                            aria-label={column.name}>
                            <div className="jira-column-head">
                                <span>{column.name}</span>
                                <span className="jira-count">{column.issues.length}</span>
                            </div>
                            {column.issues.map((issue) => (
                                <button
                                    key={issue.key}
                                    type="button"
                                    role="listitem"
                                    className={`jira-card${issue.key === selected ? " active" : ""}${dragging?.key === issue.key ? " dragging" : ""}${issue.key in moves ? " moving" : ""}`}
                                    onPointerDown={(event) => startDrag(event, issue)}
                                    onClick={() => {
                                        if (!justDragged.current) onSelect(issue.key);
                                    }}
                                    onContextMenu={(event) => {
                                        event.preventDefault();
                                        setMenu({
                                            x: event.clientX,
                                            y: event.clientY,
                                            items: board.columns.map((target, to) => ({
                                                label: `Move to ${target.name}`,
                                                disabled: to === index,
                                                run: () => void move(issue, to),
                                            })),
                                        });
                                    }}>
                                    <span className="jira-card-summary">{issue.summary}</span>
                                    <span className="jira-card-foot">
                                        <span className="jira-key">{issue.key}</span>
                                        {issue.issueType && <span className="jira-meta">{issue.issueType}</span>}
                                        {issue.priority && <span className="jira-meta">{issue.priority}</span>}
                                        <span className="jira-grow" />
                                        {issue.assignee && (
                                            <span className="jira-avatar" title={issue.assignee.name}>
                                                {initials(issue.assignee.name)}
                                            </span>
                                        )}
                                    </span>
                                </button>
                            ))}
                        </div>
                    ))}
                </div>
            )}
            {menu && <ContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={() => setMenu(null)} />}
        </section>
    );
}

function initials(name: string): string {
    return name
        .split(/\s+/)
        .filter(Boolean)
        .slice(0, 2)
        .map((part) => part[0]?.toUpperCase() ?? "")
        .join("");
}
