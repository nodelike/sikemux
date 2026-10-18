import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
    cardColumn,
    cardQuestion,
    RUN_WAITS_ON_PERSON,
    shortTitle,
    ticketAbove,
    type BoardCard,
    type BoardColumn,
    type CardStatus,
} from "../agents/agentBoard";
import { agentBoardApi } from "../api/agentBoard";
import * as cmd from "../state/commands";
import { reportError } from "../state/toast";
import { useStore } from "../state/store";
import { basename } from "../lib/paths";
import { IconBoard, IconLink, IconRefresh } from "../ui/Icons";
import { EmptyState } from "../ui/Panel";
import { useCardDrag } from "../ui/kanbanDrag";
import { Switch } from "../ui/Controls";
import { AutoPanel } from "./AutoPanel";
import { CardPanel } from "./CardPanel";
import { MessagesPanel } from "./MessagesPanel";
import { AgentMark } from "../agents/AgentMark";
import { Owner } from "./cardParts";
import "../styles/board.css";

const COLUMNS: readonly [BoardColumn, string][] = [
    ["open", "Open"],
    ["progress", "In progress"],
    ["needs", "Needs you"],
    ["done", "Done"],
];

const EMPTY: BoardCard[] = [];

function readBoard(folder: string): void {
    agentBoardApi
        .get(folder)
        .then((board) => cmd.setAgentBoard(folder, board))
        .catch(reportError("read the board"));
}

export function BoardPane({ folder, visible }: { folder: string; visible: boolean }) {
    const project = useStore((s) => s.boardProjects[folder] ?? folder);
    const cards = useStore((s) => s.agentBoards[project] ?? EMPTY);
    const auto = useStore((s) => s.agentBoardAuto[project]);
    const questions = useStore((s) => s.agentQuestions);
    const [words, setWords] = useState("");
    const [fromJira, setFromJira] = useState(false);
    const [onlyNeeds, setOnlyNeeds] = useState(false);
    const [selected, setSelected] = useState<number | "new" | "auto" | "messages" | null>(null);
    const [moving, setMoving] = useState<Record<number, CardStatus>>({});

    const refresh = () => readBoard(folder);
    useEffect(() => {
        if (visible) readBoard(folder);
    }, [visible, folder]);

    const byId = useMemo(() => new Map(cards.map((card) => [card.id, card])), [cards]);
    const asking = (card: BoardCard) => cardQuestion(card, questions);
    const columnOf = (card: BoardCard): BoardColumn => cardColumn(card, questions, moving[card.id]);

    const move = async (id: number, to: CardStatus) => {
        const card = byId.get(id);
        if (!card || card.status === to) return;
        setMoving((now) => ({ ...now, [id]: to }));
        try {
            cmd.setAgentBoard(folder, await agentBoardApi.update(folder, id, { status: to }));
        } catch (error) {
            reportError("move the card")(error);
        } finally {
            setMoving((now) => {
                const next = { ...now };
                delete next[id];
                return next;
            });
        }
    };
    const drag = useCardDrag((card, column) => {
        if (column !== "needs") void move(Number(card), column as CardStatus);
    });

    const needing = cards.filter((card) => columnOf(card) === "needs");
    const fromTickets = cards.filter((card) => card.ticket).length;
    const wanted = words.trim().toLowerCase();
    const shown = cards.filter(
        (card) =>
            (!fromJira || card.ticket) &&
            (!onlyNeeds || columnOf(card) === "needs") &&
            (!wanted || [card.title, card.details, card.ownerName ?? "", card.ticket?.key ?? ""].some((text) => text.toLowerCase().includes(wanted))),
    );

    const teams = new Map<string | null, BoardCard[]>();
    for (const card of [...shown].sort((a, b) => b.updatedAt - a.updatedAt)) {
        const list = teams.get(card.team) ?? [];
        list.push(card);
        teams.set(card.team, list);
    }
    const bands = [...teams.entries()].sort(([a], [b]) => (a === null ? 1 : b === null ? -1 : 0));
    const current = typeof selected === "number" ? byId.get(selected) : undefined;
    const panel = selected === "new" || selected === "auto" || selected === "messages" || current;
    const autoOn = auto?.settings.auto ?? false;
    const setAuto = (on: boolean) =>
        agentBoardApi
            .settings(folder, { auto: on })
            .then((board) => cmd.setAgentBoard(folder, board))
            .catch(reportError(on ? "turn Auto on" : "turn Auto off"));

    const card = (each: BoardCard) => {
        const question = columnOf(each) === "needs" ? asking(each) : undefined;
        const under = cards.filter((other) => other.parent === each.id);
        const done = under.filter((other) => other.status === "done").length;
        const waits = each.waitsOn.map((id) => byId.get(id)).filter((other): other is BoardCard => !!other);
        const after = waits[0] ?? (each.dependsOn.length > 0 ? byId.get(each.dependsOn[0]) : undefined);
        const filed = each.ticket ? null : ticketAbove(each, byId);
        const parent = each.parent === null ? undefined : byId.get(each.parent);
        return (
            <button
                key={each.id}
                type="button"
                role="listitem"
                className={`kanban-card${each.id === selected ? " active" : ""}${drag.dragging?.card === String(each.id) ? " dragging" : ""}${each.id in moving ? " moving" : ""}${each.status === "done" ? " board-card-done" : ""}`}
                onPointerDown={(event) => drag.startDrag(event, String(each.id))}
                onClick={() => {
                    if (!drag.wasDragged()) setSelected(each.id);
                }}>
                <span className="kanban-card-summary">{each.title}</span>
                {question ? (
                    <span className="board-ask">
                        <b>{question.name}</b> asks: {question.text}
                    </span>
                ) : (
                    each.auto?.note &&
                    each.status !== "done" && (
                        <span className={`board-ask${RUN_WAITS_ON_PERSON.has(each.auto.stage) ? "" : " board-run"}`}>{each.auto.note}</span>
                    )
                )}
                {(under.length > 0 || after) && (
                    <span className="board-row">
                        {under.length > 0 && (
                            <span className="board-sub" title={`${done} of ${under.length} cards under it done`}>
                                {done}/{under.length}
                            </span>
                        )}
                        {after && (
                            <span className={`board-after${waits.length > 0 ? " waiting" : ""}`} title={`After ${after.title}`}>
                                <IconLink size={11} />
                                after {shortTitle(after.title)}
                            </span>
                        )}
                    </span>
                )}
                <span className="kanban-card-foot">
                    {each.ticket ? (
                        <>
                            <span className="kanban-key">{each.ticket.key}</span>
                            {each.ticket.source && <span className="kanban-meta">{each.ticket.source}</span>}
                        </>
                    ) : filed ? (
                        <span className="kanban-meta">under {filed}</span>
                    ) : parent ? (
                        <span className="kanban-meta">under {shortTitle(parent.title)}</span>
                    ) : null}
                    <span className="kanban-grow" />
                    {each.owner ? (
                        <Owner id={each.owner} name={each.ownerName} cli={each.ownerCli} paidBy={each.paidBy} />
                    ) : (
                        <span className="board-nobody">Nobody yet</span>
                    )}
                </span>
            </button>
        );
    };

    const columns = (list: BoardCard[]) => (
        <div className="kanban-columns">
            {COLUMNS.map(([key, name]) => {
                const inColumn = list.filter((each) => columnOf(each) === key);
                return (
                    <div
                        key={key}
                        className={`kanban-column${drag.dragging?.over === key && key !== "needs" ? " drop" : ""}`}
                        data-column={key}
                        role="list"
                        aria-label={name}>
                        <div className="kanban-column-head">
                            <span>{name}</span>
                            <span className="kanban-count">{inColumn.length}</span>
                        </div>
                        {inColumn.map(card)}
                    </div>
                );
            })}
        </div>
    );

    const bandHead = (team: string | null, list: BoardCard[]): ReactNode => {
        if (team === null)
            return (
                <>
                    <b>Nobody yet</b>
                    {autoOn && <span className="kanban-meta">Auto picks these up</span>}
                </>
            );
        const keys = [...new Set(list.flatMap((each) => (each.ticket ? [each.ticket.key] : [])))];
        const workers = [...new Set(list.flatMap((each) => (each.owner && each.owner !== team && each.ownerName ? [each.ownerName] : [])))];
        return (
            <>
                <TeamMark id={team} cli={list.find((each) => each.owner === team)?.ownerCli ?? null} />
                <b>{list[0]?.teamName ?? "An agent"}'s team</b>
                {(keys.length > 0 || workers.length > 0) && (
                    <span className="kanban-meta">{[...keys, workers.join(", ")].filter(Boolean).join(" · ")}</span>
                )}
            </>
        );
    };

    return (
        <div className={`board-pane${panel ? " with-card" : ""}`}>
            <section className="kanban" aria-label="Board">
                <header className="kanban-head">
                    <div className="kanban-title">
                        <h2>Board</h2>
                        <span className="kanban-tag board-project">{basename(folder)}</span>
                        <span className="kanban-meta">
                            {cards.length === 1 ? "1 card" : `${cards.length} cards`}
                            {fromTickets > 0 && ` · ${fromTickets} from Jira`}
                        </span>
                        <span className="kanban-grow" />
                        <span className="board-auto" title={auto?.autoNote ?? undefined}>
                            <Switch checked={autoOn} onChange={(on) => void setAuto(on)} label="Auto" />
                            <button type="button" className="board-auto-label" aria-pressed={selected === "auto"} onClick={() => setSelected("auto")}>
                                {autoOn && auto ? `Auto · ${auto.running} of ${auto.settings.slots} running` : "Auto"}
                            </button>
                        </span>
                        <button
                            type="button"
                            className="board-auto-label"
                            aria-pressed={selected === "messages"}
                            title="Messages between this project's agents"
                            onClick={() => setSelected("messages")}>
                            Messages
                        </button>
                        <button type="button" className="kanban-button" onClick={() => setSelected("new")}>
                            Add card
                        </button>
                        <button type="button" className="kanban-icon-button" title="Refresh" aria-label="Refresh the board" onClick={refresh}>
                            <IconRefresh size={13} />
                        </button>
                    </div>
                    <div className="kanban-filters" role="group" aria-label="Filter the board">
                        <input
                            className="kanban-search"
                            value={words}
                            onChange={(event) => setWords(event.target.value)}
                            placeholder="Find a card"
                            aria-label="Find a card"
                            spellCheck={false}
                        />
                        <button type="button" className="kanban-pick" aria-pressed={fromJira} onClick={() => setFromJira((on) => !on)}>
                            From Jira
                        </button>
                        <button type="button" className="kanban-pick" aria-pressed={onlyNeeds} onClick={() => setOnlyNeeds((on) => !on)}>
                            Needs you <span className="kanban-pick-count">{needing.length}</span>
                        </button>
                        {(words || fromJira || onlyNeeds) && (
                            <button
                                type="button"
                                className="kanban-clear"
                                onClick={() => {
                                    setWords("");
                                    setFromJira(false);
                                    setOnlyNeeds(false);
                                }}>
                                Clear
                            </button>
                        )}
                    </div>
                </header>
                {cards.length === 0 ? (
                    <EmptyState
                        icon={<IconBoard size={14} />}
                        title="No cards yet"
                        message="Agents plan their work here with board_add, and you can add cards of your own."
                        action={{ label: "Add card", onClick: () => setSelected("new") }}
                    />
                ) : shown.length === 0 ? (
                    <EmptyState message="No card matches." />
                ) : (
                    <div className="board-bands">
                        {bands.map(([team, list]) => (
                            <div key={team ?? "nobody"} className="board-band">
                                <div className="board-band-head">{bandHead(team, list)}</div>
                                {columns(list)}
                            </div>
                        ))}
                    </div>
                )}
            </section>
            {selected === "auto" ? (
                <AutoPanel folder={folder} auto={auto ?? null} onClose={() => setSelected(null)} />
            ) : selected === "messages" ? (
                <MessagesPanel folder={folder} onClose={() => setSelected(null)} />
            ) : (
                panel && (
                    <CardPanel
                        key={selected ?? "none"}
                        folder={folder}
                        card={current ?? null}
                        cards={cards}
                        onSelect={setSelected}
                        onClose={() => setSelected(null)}
                    />
                )
            )}
        </div>
    );
}

function TeamMark({ id, cli }: { id: string; cli: string | null }) {
    const type = useStore((s) => s.agents[id]?.type);
    return <AgentMark cli={type ?? cli} />;
}
