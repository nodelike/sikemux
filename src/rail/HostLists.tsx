import { useMemo } from "react";
import { useShortcutLabel } from "../commands/useShortcutLabel";
import { hostTitle, openRemoteChat, openRemoteTerminal, placeOn, showTab, type HostChoice } from "../hosts/hosts";
import { collectPanes } from "../state/layout";
import { terminalLabel } from "../state/selectors";
import { getState, useStore, type StoreState } from "../state/store";
import { IconTerminal } from "@shared/ui/Icons";
import { Panel } from "../ui/Panel";
import { windowTabMenu } from "../workspace/tabMenus";
import { TabRow, useTabMenu } from "./TabRow";
import { RailEmpty } from "./RailEmpty";
import { RemoteChatRow, RemoteTerminalRow } from "./RemoteRows";

export type Place = Pick<StoreState, "hostViews" | "sessionOrder" | "sessions" | "windowsBySession" | "windows">;

export function usePlace(): Place {
    const hostViews = useStore((s) => s.hostViews);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const sessions = useStore((s) => s.sessions);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const windows = useStore((s) => s.windows);
    return useMemo(
        () => ({ hostViews, sessionOrder, sessions, windowsBySession, windows }),
        [hostViews, sessionOrder, sessions, windowsBySession, windows],
    );
}

/** The terminal tabs open here that run on the chosen host, from every session. */
function openTerminals(state: Place, choice: HostChoice) {
    return state.sessionOrder.flatMap((id) => {
        const owner = state.sessions[id];
        if (!owner) return [];
        const onHost = choice.kind === "ssh" ? owner.kind === "ssh" && owner.name === choice.alias : owner.kind !== "ssh";
        if (!onHost) return [];
        return (state.windowsBySession[id] ?? []).flatMap((windowId) => {
            const win = state.windows[windowId];
            if (!win || (choice.kind === "core" && win.role !== "term")) return [];
            const home = collectPanes(win.root).find((pane) => pane.kind === "terminal");
            if (!home || (choice.kind === "core" && home.host !== choice.host)) return [];
            return [{ win, owner, home }];
        });
    });
}

/** Every terminal on the chosen host, from every project. */
export function HostTerminals({ choice }: { choice: HostChoice }) {
    const place = usePlace();
    const titles = useStore((s) => s.terminalTitles);
    const shown = useStore((s) => s.sessions[s.activeSessionId]?.activeWindowId);
    const name = useStore((s) => hostTitle(s, choice));
    const closeShortcut = useShortcutLabel("pane.close");
    const menu = useTabMenu();
    const host = choice.kind === "core" ? choice.host : undefined;
    const open = openTerminals(place, choice);
    const showing = new Set(open.flatMap(({ home }) => home.ptyId ?? []));
    const running = host
        ? (place.hostViews[host]?.sessions ?? []).filter(
              (session) => session.kind === "terminal" && session.running && !session.agentId && !showing.has(session.id),
          )
        : [];

    if (open.length + running.length === 0)
        return <RailEmpty icon={<IconTerminal size={20} />} title={`No terminals on ${name}`} shortcut="terminal.new" doing="start one" />;
    return (
        <Panel variant="group" className="agent-group">
            {open.map(({ win, owner, home }) => (
                <TabRow
                    key={win.id}
                    tab={{ id: win.id }}
                    icon={<IconTerminal size={15} />}
                    glyph="term"
                    label={terminalLabel(win, home, titles)}
                    project={owner.kind === "project" ? { name: owner.name, sessionId: owner.id } : undefined}
                    active={shown === win.id}
                    onOpen={() => showTab(win.id)}
                    onMenu={(event) => menu.open(event, windowTabMenu(owner.id, getState().windows[win.id] ?? win, closeShortcut))}
                />
            ))}
            {host &&
                running.map((terminal) => {
                    const { project, sessionId } = placeOn(place, host, terminal.project);
                    return (
                        <RemoteTerminalRow
                            key={terminal.id}
                            terminal={terminal}
                            project={project?.name}
                            onOpen={() => openRemoteTerminal(sessionId, host, terminal, project?.path ?? "")}
                        />
                    );
                })}
            {menu.shown}
        </Panel>
    );
}

/** The chats running on `host` that this app does not show yet, from every project, leaving out those in `shown`. */
export function runningChats(place: Place, agents: StoreState["agents"], host: string, shown: ReadonlySet<string>) {
    return (place.hostViews[host]?.chats ?? [])
        .filter((chat) => !agents[chat.agentId] && !shown.has(chat.agentId))
        .map((chat) => {
            const { project, sessionId } = placeOn(place, host, chat.cwd);
            return <RemoteChatRow key={chat.agentId} chat={chat} project={project?.name} onOpen={() => openRemoteChat(sessionId, host, chat)} />;
        });
}
