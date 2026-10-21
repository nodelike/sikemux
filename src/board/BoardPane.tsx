import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
    cardColumn,
    cardQuestion,
    COLUMN_LABELS,
    RUN_WAITS_ON_PERSON,
    shortTitle,
    type BoardCard,
    type BoardColumn,
    type CardStatus,
} from "../agents/agentBoard";
import { agentBoardApi } from "../api/agentBoard";
import * as cmd from "../state/commands";
import { reportError } from "../state/toast";
import { useStore } from "../state/store";
import { IconBoard, IconChevron, IconLink, IconPlus, IconSearch } from "../ui/Icons";
import { EmptyState } from "../ui/Panel";
import { useCardDrag } from "../ui/kanbanDrag";
import { Segmented } from "../ui/Controls";
import { AutoPanel } from "./AutoPanel";
import { CardPanel } from "./CardPanel";
import { MessagesPanel } from "./MessagesPanel";
import { Owner } from "./cardParts";
import "../styles/board.css";

const COLUMNS: readonly BoardColumn[] = ["open", "progress", "needs", "done"];

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
    const [show, setShow] = useState<"all" | "jira" | "needs">("all");
    const [selected, setSelected] = useState<number | "new" | "auto" | "messages" | null>(null);
    const [moving, setMoving] = useState<Record<number, CardStatus>>({});
    const [folded, setFolded] = useState<ReadonlySet<string>>(new Set());
    const fold = (lane: string) =>
        setFolded((now) => {
            const next = new Set(now);
            if (!next.delete(lane)) next.add(lane);
            return next;
        });

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
            (show !== "jira" || card.ticket) &&
            (show !== "needs" || columnOf(card) === "needs") &&
            (!wanted || [card.title, card.details, card.ownerName ?? "", card.ticket?.key ?? ""].some((text) => text.toLowerCase().includes(wanted))),
    );

    const lanes = useMemo(() => cardLanes(cards, shown, byId), [cards, shown, byId]);
    const current = typeof selected === "number" ? byId.get(selected) : undefined;
    const panel = selected === "new" || selected === "auto" || selected === "messages" || current;
    const autoOn = auto?.settings.auto ?? false;

    const card = (each: BoardCard) => {
        const question = columnOf(each) === "needs" ? asking(each) : undefined;
        const under = cards.filter((other) => other.parent === each.id);
        const done = under.filter((other) => other.status === "done").length;
        const waits = each.waitsOn.map((id) => byId.get(id)).filter((other): other is BoardCard => !!other);
        const after = waits[0] ?? (each.dependsOn.length > 0 ? byId.get(each.dependsOn[0]) : undefined);
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
                {(each.ticket || each.owner) && (
                    <span className="kanban-card-foot">
                        {each.ticket && (
                            <>
                                <span className="kanban-key">{each.ticket.key}</span>
                                {each.ticket.source && <span className="kanban-meta">{each.ticket.source}</span>}
                            </>
                        )}
                        <span className="kanban-grow" />
                        {each.owner && <Owner id={each.owner} name={each.ownerName} cli={each.ownerCli} paidBy={each.paidBy} />}
                    </span>
                )}
            </button>
        );
    };

    const columns = (list: BoardCard[]) => (
        <div className="kanban-columns">
            {COLUMNS.map((key) => {
                const name = COLUMN_LABELS[key];
                const inColumn = list.filter((each) => columnOf(each) === key);
                return (
                    <div
                        key={key}
                        className="kanban-column"
                        data-column={key}
                        data-drop={key === "needs" ? "false" : undefined}
                        role="list"
                        aria-label={name}>
                        <div className="kanban-column-head">
                            <span className="kanban-column-name">{name}</span>
                            <span className="kanban-count">{inColumn.length}</span>
                        </div>
                        {inColumn.map(card)}
                    </div>
                );
            })}
        </div>
    );

    const laneHead = ({ parent, cards: list }: Lane): ReactNode => {
        if (!parent)
            return (
                <span className="board-lane-title">
                    <b>On their own</b>
                    <span className="kanban-meta">{autoOn ? `${count(list)} · Auto picks up the open ones` : count(list)}</span>
                </span>
            );
        const under = cards.filter((other) => other.parent === parent.id);
        const done = under.filter((other) => other.status === "done").length;
        const above = parent.parent === null ? undefined : byId.get(parent.parent);
        return (
            <button type="button" className="board-lane-title" aria-pressed={selected === parent.id} onClick={() => setSelected(parent.id)}>
                <b>{parent.title}</b>
                <span className="kanban-meta">
                    {[`${done} of ${under.length} done`, parent.ticket?.key, above && `under ${shortTitle(above.title)}`].filter(Boolean).join(" · ")}
                </span>
                {parent.owner && <Owner id={parent.owner} name={parent.ownerName} cli={parent.ownerCli} paidBy={parent.paidBy} />}
            </button>
        );
    };

    return (
        <div className={`board-pane${panel ? " with-card" : ""}`}>
            <section className="kanban" aria-label="Agent board">
                <header className="kanban-head">
                    <div className="kanban-title">
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
                        <Segmented
                            label="Which cards to show"
                            value={show}
                            options={[
                                { value: "all", label: "All", count: cards.length },
                                ...(fromTickets > 0 ? [{ value: "jira" as const, label: "From Jira", count: fromTickets }] : []),
                                { value: "needs", label: "Needs you", count: needing.length },
                            ]}
                            onChange={setShow}
                        />
                        <span className="kanban-grow" />
                        <button
                            type="button"
                            className="kanban-pick"
                            aria-pressed={selected === "auto"}
                            title={auto?.autoNote ?? (autoOn ? "Auto is on" : "Auto is off")}
                            onClick={() => setSelected("auto")}>
                            <span className={`board-auto-dot${autoOn ? " on" : ""}`} />
                            Auto
                            {autoOn && auto && (
                                <span className="kanban-pick-count">
                                    {auto.running} of {auto.settings.slots}
                                </span>
                            )}
                        </button>
                        <button
                            type="button"
                            className="kanban-pick"
                            aria-pressed={selected === "messages"}
                            title="Messages between this project's agents"
                            onClick={() => setSelected("messages")}>
                            Messages
                        </button>
                        <button type="button" className="kanban-pick" onClick={() => setSelected("new")}>
                            <IconPlus size={12} />
                            Add card
                        </button>
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
                    <div className="kanban-lanes">
                        {lanes.map((lane) => (
                            <div key={lane.key} className="kanban-lane">
                                <div className="kanban-lane-head board-lane-head">
                                    <button
                                        type="button"
                                        className="kanban-icon-button"
                                        aria-expanded={!folded.has(lane.key)}
                                        aria-label={folded.has(lane.key) ? "Show its cards" : "Hide its cards"}
                                        onClick={() => fold(lane.key)}>
                                        <IconChevron size={12} className="kanban-lane-chevron" />
                                    </button>
                                    {laneHead(lane)}
                                </div>
                                {!folded.has(lane.key) && columns(lane.cards)}
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

const count = (list: readonly BoardCard[]) => (list.length === 1 ? "1 card" : `${list.length} cards`);

interface Lane {
    key: string;
    parent: BoardCard | null;
    cards: BoardCard[];
}

/** One lane per card that has cards under it, holding them, then one for the cards on their own; the newest work first. */
function cardLanes(cards: readonly BoardCard[], shown: readonly BoardCard[], byId: ReadonlyMap<number, BoardCard>): Lane[] {
    const parents = new Set(cards.flatMap((card) => (card.parent === null ? [] : [card.parent])));
    const lanes = new Map<string, Lane>();
    const laneOf = (parent: BoardCard | null): Lane => {
        const key = parent ? `card-${parent.id}` : "alone";
        let lane = lanes.get(key);
        if (!lane) lanes.set(key, (lane = { key, parent, cards: [] }));
        return lane;
    };
    for (const card of [...shown].sort((a, b) => b.updatedAt - a.updatedAt)) {
        if (parents.has(card.id)) laneOf(card);
        else laneOf((card.parent === null ? undefined : byId.get(card.parent)) ?? null).cards.push(card);
    }
    const alone = lanes.get("alone");
    lanes.delete("alone");
    return alone ? [...lanes.values(), alone] : [...lanes.values()];
}
