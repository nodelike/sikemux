import { Fragment, useLayoutEffect, useMemo, type ReactNode, type RefObject } from "react";
import { AgentStateIndicator } from "../agents/AgentStateIndicator";
import { AgentFace } from "../buddies/Buddy";
import type { TrackedItem } from "../codehost/tracked";
import { formatAge } from "@shared/lib/format";
import { dirname } from "@shared/lib/paths";
import { useResourceEnabled } from "../state/resources";
import { filesListR } from "../state/resources.defs";
import { FileIcon } from "../ui/FileIcon";
import { AgentIcon, IconFolder } from "@shared/ui/Icons";
import type { Mate } from "./agentMentions";
import {
    commandMatches,
    entryName,
    MAX_MENU_ENTRIES,
    projectEntries,
    rankEntries,
    type ComposerToken,
    type ComposerTrigger,
    type ProjectEntry,
} from "@shared/chat/composerInput";
import { ContextMark } from "./ContextChip";
import { earlierUri, useClosedNamed, useEarlierChats, type EarlierChat } from "./earlierChats";
import type { TrackedMatches } from "./TrackedSource";
import type { ChatCommandInfo } from "@shared/chat/types";

function matchMates(mates: readonly Mate[], needle: string, limit: number): Mate[] {
    const wanted = needle.toLowerCase();
    return mates.filter((mate) => mate.name.toLowerCase().startsWith(wanted)).slice(0, limit);
}

/** What the person picked from the menu. */
export type MenuPick =
    | { kind: "command"; command: ChatCommandInfo }
    | { kind: "mate"; mate: Mate }
    | { kind: "earlier"; earlier: EarlierChat; uri: string }
    | { kind: "entry"; entry: ProjectEntry }
    | { kind: "tracked"; item: TrackedItem };

/** How the composer's keys drive the menu while it is open. */
export interface MenuKeys {
    count: number;
    choose: (index: number) => void;
}

interface MenuRow {
    key: string;
    content: ReactNode;
    pick: MenuPick;
    /** The heading it and the rows after it sit under, below the menu's own. */
    group?: string;
}

interface MenuModel {
    label: string;
    aside: string;
    variant: "commands" | "entries";
    rows: MenuRow[];
    empty: string | null;
    loading: boolean;
}

/** The prefixes that name something, in the order the menu's heading lists them. */
const SCOPES: readonly { trigger: ComposerTrigger; label: string }[] = [
    { trigger: "@", label: "Agents" },
    { trigger: "#", label: "Issues" },
    { trigger: "!", label: "Pull requests" },
    { trigger: "+", label: "Files" },
];

const LOADING_ROWS = [72, 54, 63];

const agentMark = (type: Mate["type"]) => (
    <span className="chat-menu-mark">
        <span className={`agent-glyph ${type}`}>
            <AgentIcon type={type} size={13} />
        </span>
    </span>
);

function trackedRows(tracked: TrackedMatches, prefix: string): MenuRow[] {
    if (tracked.state !== "ready") return [];
    return tracked.matches.map((item) => ({
        key: `${item.kind}-${item.number}`,
        pick: { kind: "tracked", item },
        content: (
            <>
                <span className="chat-menu-mark" data-kind={item.kind}>
                    <ContextMark kind={item.kind} size={13} />
                </span>
                <code>
                    {prefix}
                    {item.number}
                </code>
                <span>{item.title}</span>
            </>
        ),
    }));
}

/** The menu docked on the composer for the prefix the caret is in. */
export default function ComposerMenu({
    token,
    agentId,
    cwd,
    commands,
    mates,
    taken,
    tracked,
    selected,
    keys,
    onPick,
    onScope,
    peek,
}: {
    token: ComposerToken;
    agentId: string;
    cwd: string;
    commands: readonly ChatCommandInfo[];
    mates: readonly Mate[];
    /** The uris of what the message already carries. */
    taken: readonly string[];
    tracked: TrackedMatches;
    selected: number;
    keys: RefObject<MenuKeys | null>;
    onPick: (pick: MenuPick) => void;
    onScope: (trigger: ComposerTrigger) => void;
    /** Shown peeking over the menu's top edge. */
    peek?: ReactNode;
}) {
    const { trigger, needle } = token;
    const slashCommands = useMemo(() => (trigger === "/" ? commandMatches(commands, needle) : []), [commands, trigger, needle]);
    const files = useResourceEnabled(trigger === "+" && !!cwd, filesListR, cwd);
    const entries = useMemo(() => projectEntries(files.data ?? []), [files.data]);
    const entryMatches = useMemo(
        () => (trigger === "+" && cwd ? rankEntries(needle, entries, MAX_MENU_ENTRIES) : []),
        [entries, cwd, trigger, needle],
    );
    const mateMatches = useMemo(() => (trigger === "@" ? matchMates(mates, needle, MAX_MENU_ENTRIES) : []), [mates, trigger, needle]);
    const closedNamed = useClosedNamed(agentId, trigger === "@" ? needle : null).filter((earlier) => !taken.includes(earlierUri(earlier)));
    const earlierMatches = useEarlierChats(agentId, trigger === "@" ? needle : null).filter(
        (earlier) => !taken.includes(earlierUri(earlier)) && !closedNamed.some((named) => earlierUri(named) === earlierUri(earlier)),
    );
    const trackedEmpty = tracked.state === "unavailable" ? tracked.message : null;

    const menu: MenuModel | null =
        trigger === "/"
            ? slashCommands.length > 0
                ? {
                      label: "Session commands",
                      aside: "ACP",
                      variant: "commands",
                      empty: null,
                      loading: false,
                      rows: slashCommands.map((command) => ({
                          key: command.name,
                          pick: { kind: "command", command },
                          content: (
                              <>
                                  <code>/{command.name}</code>
                                  <span>{command.description}</span>
                                  {command.hint && <em>{command.hint}</em>}
                              </>
                          ),
                      })),
                  }
                : null
            : trigger === "@"
              ? {
                    label: "Agents and earlier chats",
                    aside: "mention",
                    variant: "entries",
                    empty: mates.length === 0 && closedNamed.length === 0 && !needle ? "No other agent yet" : "No agent or earlier chat matches",
                    loading: false,
                    rows: [
                        ...mateMatches.map((mate): MenuRow => ({
                            key: `agent:${mate.id}`,
                            group: mate.project ?? undefined,
                            pick: { kind: "mate", mate },
                            content: (
                                <>
                                    <span className="chat-menu-mark">
                                        <AgentFace
                                            agent={mate}
                                            size={13}
                                            logoSize={13}
                                            logoClass={`agent-glyph ${mate.type}`}
                                            buddyClass="agent-glyph buddy-mark"
                                        />
                                    </span>
                                    <code>@{mate.name}</code>
                                    <span className="chat-menu-mate">
                                        <span>{mate.title}</span>
                                        {mate.display && <AgentStateIndicator display={mate.display} />}
                                    </span>
                                </>
                            ),
                        })),
                        ...closedNamed.slice(0, MAX_MENU_ENTRIES).map((earlier): MenuRow => ({
                            key: earlierUri(earlier),
                            group: "Closed",
                            pick: { kind: "earlier", earlier, uri: earlierUri(earlier) },
                            content: (
                                <>
                                    <span className="chat-menu-mark">
                                        <AgentFace
                                            agent={{
                                                id: earlier.named?.agentId ?? earlier.chat.id,
                                                type: earlier.chat.agent,
                                                buddy: earlier.named?.buddy,
                                            }}
                                            size={13}
                                            logoSize={13}
                                            logoClass={`agent-glyph ${earlier.chat.agent}`}
                                            buddyClass="agent-glyph buddy-mark"
                                        />
                                    </span>
                                    <code>@{earlier.named?.name}</code>
                                    <span className="chat-menu-mate">
                                        <span>{earlier.chat.title}</span>
                                    </span>
                                </>
                            ),
                        })),
                        ...earlierMatches.map((earlier): MenuRow => ({
                            key: earlierUri(earlier),
                            group: "Earlier chats",
                            pick: { kind: "earlier", earlier, uri: earlierUri(earlier) },
                            content: (
                                <>
                                    {agentMark(earlier.chat.agent)}
                                    <span className="chat-menu-earlier">{earlier.chat.title}</span>
                                    <em>{formatAge(earlier.chat.mtime * 1000)}</em>
                                </>
                            ),
                        })),
                    ],
                }
              : trigger === "+"
                ? {
                      label: "Project files",
                      aside: "attach",
                      variant: "entries",
                      empty: cwd ? "Nothing in the project matches" : "Open a project to pick its files",
                      loading: !!cwd && files.data === undefined,
                      rows: entryMatches.map((entry): MenuRow => ({
                          key: entry.path,
                          pick: { kind: "entry", entry },
                          content: (
                              <>
                                  <span className="chat-menu-mark">
                                      {entry.folder ? <IconFolder size={13} /> : <FileIcon name={entryName(entry)} size={13} />}
                                  </span>
                                  <code>{entryName(entry)}</code>
                                  <span>{dirname(entry.path.replace(/\/$/, "")) || "."}</span>
                              </>
                          ),
                      })),
                  }
                : {
                      label: trigger === "#" ? "Issues" : "Pull requests",
                      aside: tracked.state === "ready" ? `${tracked.repo.owner}/${tracked.repo.name}` : "",
                      variant: "entries",
                      empty: trackedEmpty ?? (trigger === "#" ? "No open issue matches" : "No open pull request matches"),
                      loading: tracked.state === "loading",
                      rows: trackedRows(tracked, trigger),
                  };
    const rows = menu?.rows ?? [];
    const shown = Math.min(selected, Math.max(0, rows.length - 1));

    useLayoutEffect(() => {
        keys.current = menu ? { count: rows.length, choose: (index) => onPick(rows[index].pick) } : null;
    });
    useLayoutEffect(
        () => () => {
            keys.current = null;
        },
        [keys],
    );

    return (
        menu && (
            <div className="chat-slash-dock">
                {peek}
                <div className={`chat-slash-menu ${menu.variant}`} role="listbox" aria-label={menu.label} aria-busy={menu.loading}>
                    <div className="chat-slash-heading">
                        {trigger === "/" ? (
                            <span>Session commands</span>
                        ) : (
                            <span className="chat-menu-scopes">
                                {SCOPES.map((scope) => (
                                    <button
                                        key={scope.trigger}
                                        type="button"
                                        aria-pressed={scope.trigger === trigger}
                                        onMouseDown={(event) => event.preventDefault()}
                                        onClick={() => onScope(scope.trigger)}>
                                        {menu.loading && scope.trigger === trigger ? (
                                            <span className="chat-subagent-spinner" aria-hidden="true" />
                                        ) : (
                                            <kbd>{scope.trigger}</kbd>
                                        )}
                                        {scope.label}
                                    </button>
                                ))}
                            </span>
                        )}
                        <span>{menu.aside}</span>
                    </div>
                    {menu.loading &&
                        rows.length === 0 &&
                        LOADING_ROWS.map((width) => (
                            <div key={width} className="chat-menu-loading" aria-hidden="true">
                                <i />
                                <i />
                                <i style={{ width: `${width}%` }} />
                            </div>
                        ))}
                    {!menu.loading && rows.length === 0 && menu.empty && <div className="chat-slash-empty">{menu.empty}</div>}
                    {rows.map((row, index) => (
                        <Fragment key={row.key}>
                            {row.group && row.group !== rows[index - 1]?.group && <div className="chat-slash-group">{row.group}</div>}
                            <button
                                type="button"
                                role="option"
                                aria-selected={index === shown}
                                className={index === shown ? "selected" : ""}
                                onMouseDown={(event) => event.preventDefault()}
                                onClick={() => onPick(row.pick)}>
                                {row.content}
                            </button>
                        </Fragment>
                    ))}
                </div>
            </div>
        )
    );
}
