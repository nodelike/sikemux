import { useEffect, useRef, useState, type MouseEvent, type PointerEvent } from "react";
import { notify } from "../../../plugin-api/host";
import { errorText } from "../../../plugin-api/format";
import { useResourceEnabled } from "../../../plugin-api/resources";
import {
    ContextMenu,
    EmptyState,
    IconChevron,
    IconMove,
    IconRefresh,
    IconSearch,
    Pending,
    StaleNotice,
    useCardDrag,
    type ContextMenuItem,
} from "../../../plugin-api/ui";
import { jiraApi, type JiraColumn, type JiraIssueSummary } from "../api";
import { matchingCards, pickKey } from "../boardFilter";
import { jira, jiraBoardV } from "../resources";
import { Avatar, JiraFace } from "./JiraFace";
import { DueChip, EpicChip, IssueKey, IssueTypeIcon, PriorityGlyph } from "./JiraGlyphs";

const FACES = 6;

/** Where each card sits after the moves still on their way to Jira, by issue key and column index. */
export function withMoves(columns: JiraColumn[], moves: Record<string, number>): JiraColumn[] {
    const moving = new Set(Object.keys(moves));
    if (moving.size === 0) return columns;
    return columns.map((column, index) => ({
        ...column,
        issues: [
            ...column.issues.filter((issue) => !moving.has(issue.key)),
            ...columns.flatMap((other) => other.issues.filter((issue) => moves[issue.key] === index)),
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

export function JiraBoard({
    paneId,
    active,
    boardId,
    site,
    selected,
    onSelect,
}: {
    paneId: string;
    active: boolean;
    boardId: number;
    site: string;
    selected: string | null;
    onSelect: (key: string) => void;
}) {
    const [picked, setPicked] = useState<ReadonlySet<string>>(new Set());
    const [words, setWords] = useState("");
    const [everyone, setEveryone] = useState(false);
    const [folded, setFolded] = useState<ReadonlySet<string>>(new Set());
    const found = useResourceEnabled(active, jiraBoardV, paneId, boardId, site, pickKey(picked));
    const [moves, setMoves] = useState<Record<string, number>>({});
    // A card moved here stays where it was dropped until the core sends the board again.
    useEffect(() => setMoves({}), [found.data]);
    // Picking people reads the board again; the last one stays up until it arrives.
    const last = useRef(found.data);
    if (found.data) last.current = found.data;
    const [menu, setMenu] = useState<{ x: number; y: number; items: ContextMenuItem[] } | null>(null);
    const drag = useCardDrag((key, column) => {
        const issue = columns.flatMap((each) => each.issues).find((each) => each.key === key);
        if (issue) void move(issue, Number(column));
    });

    const page = found.data ?? last.current;
    if (!page) return <Pending handle={found} loading="Loading the board" />;
    const { board } = page;
    const shownColumns = (columns: JiraColumn[]) => matchingCards(withMoves(columns, moves), words);
    const columns = shownColumns(board.columns);
    const refresh = () => void jira.readAgain("boardView");
    const toggle = (who: string) =>
        setPicked((now) => {
            const next = new Set(now);
            if (!next.delete(who)) next.add(who);
            return next;
        });
    const nobody = page.unassigned;
    const people = page.people;
    const hidden = everyone ? 0 : Math.max(0, people.length - FACES);
    const faces = people.filter((person, index) => index < people.length - hidden || picked.has(person.accountId));

    const move = async (issue: JiraIssueSummary, to: number) => {
        const from = columns.findIndex((column) => column.issues.some((each) => each.key === issue.key));
        const target = board.columns[to];
        if (from === to || !target) return;
        setMoves((now) => ({ ...now, [issue.key]: to }));
        try {
            await jiraApi.moveIssue(issue.key, target, site || undefined);
        } catch (error) {
            notify("error", `Move ${issue.key}: ${errorText(error)}`);
            setMoves((now) => {
                const next = { ...now };
                delete next[issue.key];
                return next;
            });
        }
    };

    const left = sprintLeft(board.sprint?.end ?? null);
    const lanes = page.lanes;
    const fold = (id: string) =>
        setFolded((now) => {
            const next = new Set(now);
            if (!next.delete(id)) next.add(id);
            return next;
        });
    const lanesOf = (shown: JiraColumn[]) => (
        <div className="kanban-columns">
            {shown.map((column, index) => (
                <div key={column.name} className="kanban-column" data-column={index} role="list" aria-label={column.name}>
                    <div className="kanban-column-head">
                        <span className="kanban-column-name">{column.name}</span>
                        <span className="kanban-count">{column.issues.length}</span>
                    </div>
                    {column.issues.map((issue) => (
                        <Card
                            key={issue.key}
                            issue={issue}
                            selected={issue.key === selected}
                            dragging={drag.dragging?.card === issue.key}
                            moving={issue.key in moves}
                            onPointerDown={(event) => drag.startDrag(event, issue.key)}
                            onClick={() => {
                                if (!drag.wasDragged()) onSelect(issue.key);
                            }}
                            onContextMenu={(event) => {
                                event.preventDefault();
                                setMenu({
                                    x: event.clientX,
                                    y: event.clientY,
                                    items: board.columns.map((target, to) => ({
                                        label: `Move to ${target.name}`,
                                        icon: <IconMove size={13} />,
                                        disabled: to === index,
                                        run: () => void move(issue, to),
                                    })),
                                });
                            }}
                        />
                    ))}
                </div>
            ))}
        </div>
    );

    return (
        <section className="kanban" aria-label={board.name}>
            <header className="kanban-head">
                <div className="kanban-title">
                    <h2>{board.name}</h2>
                    <span className="kanban-tag">{board.kind}</span>
                    {board.sprint && (
                        <span className="kanban-meta">
                            {board.sprint.name}
                            {left && ` · ${left}`}
                        </span>
                    )}
                    <span className="kanban-grow" />
                    <button type="button" className="kanban-icon-button" title="Refresh" aria-label="Refresh the board" onClick={refresh}>
                        <IconRefresh size={13} />
                    </button>
                </div>
                {board.sprint?.goal && <p className="kanban-goal">{board.sprint.goal}</p>}
                {board.truncated && (
                    <p className="kanban-meta">Showing the first 300 issues. Pick people or narrow the board in Jira to see the rest.</p>
                )}
                <div className="kanban-filters" role="group" aria-label="Filter the board">
                    <span className="kanban-find">
                        <IconSearch size={13} />
                        <input
                            className="kanban-search"
                            value={words}
                            onChange={(event) => setWords(event.target.value)}
                            placeholder="Find a card"
                            aria-label="Find a card"
                            spellCheck={false}
                        />
                    </span>
                    <span className="jira-board-people">
                        {faces.map((person) => (
                            <button
                                key={person.accountId}
                                type="button"
                                className="jira-avatar jira-avatar-pick"
                                title={`${person.name} · ${person.count}`}
                                aria-label={person.name}
                                aria-pressed={picked.has(person.accountId)}
                                onClick={() => toggle(person.accountId)}>
                                <JiraFace person={person} />
                            </button>
                        ))}
                        {hidden > 0 && (
                            <button
                                type="button"
                                className="jira-avatar jira-avatar-pick jira-avatar-more"
                                title={`Show ${hidden} more`}
                                aria-label={`Show ${hidden} more`}
                                onClick={() => setEveryone(true)}>
                                +{hidden}
                            </button>
                        )}
                    </span>
                    <button type="button" className="kanban-pick" aria-pressed={picked.has("me")} onClick={() => toggle("me")}>
                        Only my issues
                    </button>
                    {nobody > 0 && (
                        <button type="button" className="kanban-pick" aria-pressed={picked.has("none")} onClick={() => toggle("none")}>
                            Unassigned <span className="kanban-pick-count">{nobody}</span>
                        </button>
                    )}
                    {(picked.size > 0 || words) && (
                        <button
                            type="button"
                            className="kanban-clear"
                            onClick={() => {
                                setPicked(new Set());
                                setWords("");
                            }}>
                            Clear
                        </button>
                    )}
                </div>
            </header>
            <StaleNotice handle={found} />
            {board.kind === "scrum" && !board.sprint ? (
                <EmptyState message="No sprint is running on this board." />
            ) : (
                <div className="kanban-lanes">
                    {lanes.length === 0
                        ? lanesOf(columns)
                        : lanes.map((lane) => (
                              <div key={lane.id} className="kanban-lane">
                                  <button
                                      type="button"
                                      className="kanban-lane-head"
                                      aria-expanded={!folded.has(lane.id)}
                                      onClick={() => fold(lane.id)}>
                                      <IconChevron size={12} className="kanban-lane-chevron" />
                                      <Avatar person={lane.person} large />
                                      <b>{lane.person?.name ?? "Unassigned"}</b>
                                      <span className="jira-meta">({lane.count === 1 ? "1 work item" : `${lane.count} work items`})</span>
                                  </button>
                                  {!folded.has(lane.id) && lanesOf(shownColumns(lane.columns))}
                              </div>
                          ))}
                </div>
            )}
            {menu && <ContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={() => setMenu(null)} />}
        </section>
    );
}

function Card({
    issue,
    selected,
    dragging,
    moving,
    onPointerDown,
    onClick,
    onContextMenu,
}: {
    issue: JiraIssueSummary;
    selected: boolean;
    dragging: boolean;
    moving: boolean;
    onPointerDown: (event: PointerEvent) => void;
    onClick: () => void;
    onContextMenu: (event: MouseEvent) => void;
}) {
    return (
        <button
            type="button"
            role="listitem"
            className={`kanban-card${selected ? " active" : ""}${dragging ? " dragging" : ""}${moving ? " moving" : ""}`}
            onPointerDown={onPointerDown}
            onClick={onClick}
            onContextMenu={onContextMenu}>
            <span className="kanban-card-summary">{issue.summary}</span>
            {(issue.parent || issue.due) && (
                <span className="jira-card-chips">
                    {issue.parent && <EpicChip parent={issue.parent} />}
                    {issue.due && <DueChip due={issue.due} done={issue.statusCategory === "done"} />}
                </span>
            )}
            {issue.priority && (
                <span className="jira-card-marks">
                    <PriorityGlyph priority={issue.priority} />
                </span>
            )}
            <span className="kanban-card-foot">
                <IssueTypeIcon type={issue.issueType} />
                <IssueKey issue={issue} className="kanban-key" />
                <span className="kanban-grow" />
                <Avatar person={issue.assignee} large />
            </span>
        </button>
    );
}
