import type { ReactNode } from "react";
import { prettyPath } from "../lib/paths";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import type { KeyModifier, SessionKind } from "../state/types";
import { pluginSurface } from "../plugins/registry";
import { IconCommand, IconFolder } from "./Icons";

function kindIcon(kind: SessionKind): ReactNode {
    if (kind === "project") return <IconFolder size={16} />;
    const surface = pluginSurface(kind);
    if (surface) return surface.icon(16);
    return <IconCommand size={16} />;
}

function modifierLabel(modifier: KeyModifier): string {
    if (modifier === "Alt") return "⌥";
    if (modifier === "Meta") return "⌘";
    if (modifier === "Control") return "⌃";
    return "⇧";
}

export function SessionSwitcher() {
    const switcher = useStore((s) => s.sessionSwitcher);
    const sessionsById = useStore((s) => s.sessions);
    const home = useStore((s) => s.home);
    if (!switcher) return null;

    const sessions = switcher.sessionIds.map((id) => sessionsById[id]).filter(Boolean);
    const modifier = modifierLabel(switcher.releaseModifier);

    return (
        <div className="session-switcher-backdrop">
            <div className="session-switcher" role="listbox" aria-label="Switch session">
                <div className="session-switcher-head">
                    <span>Switch session</span>
                    <span className="session-switcher-hint">release {modifier} to open</span>
                </div>
                <div className="session-switcher-list">
                    {sessions.map((session) => {
                        const selected = session.id === switcher.selectedSessionId;
                        const sub = session.cwd ? prettyPath(session.cwd, home) : session.kind;
                        return (
                            <button
                                key={session.id}
                                type="button"
                                role="option"
                                aria-selected={selected}
                                className={`session-switcher-item${selected ? " selected" : ""}`}
                                onMouseDown={(event) => event.preventDefault()}
                                onClick={() => cmd.selectSession(session.id)}>
                                <span className={`session-switcher-icon ${session.kind}`}>{kindIcon(session.kind)}</span>
                                <span className="session-switcher-copy">
                                    <span className="session-switcher-name">{session.name}</span>
                                    <span className="session-switcher-sub">{sub}</span>
                                </span>
                            </button>
                        );
                    })}
                </div>
                <div className="session-switcher-foot">
                    <span>{modifier} Tab</span>
                    <span>cycle</span>
                    <span>Esc</span>
                    <span>cancel</span>
                </div>
            </div>
        </div>
    );
}
