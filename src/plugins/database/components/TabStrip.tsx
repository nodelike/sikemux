import { useState } from "react";
import { ContextMenu, IconClose, IconPlus, type ContextMenuItem } from "../../../plugin-api/ui";
import type { DatabaseProfile } from "../api";
import { tabLabel, type DatabaseTab } from "../tabs";

const GLYPH: Record<DatabaseTab["kind"], string> = { console: "›_", table: "▦", history: "↺", connection: "ⓘ" };

/** The open consoles, tables and histories, from any connection, a tab each. */
export function TabStrip({
    tabs,
    active,
    profiles,
    onSelect,
    onClose,
    onCloseOthers,
    onNewConsole,
}: {
    tabs: DatabaseTab[];
    active: string | null;
    profiles: DatabaseProfile[];
    onSelect: (id: string) => void;
    onClose: (id: string) => void;
    onCloseOthers: (id: string) => void;
    /** Opens a console on the connection in front; absent when no connection is open. */
    onNewConsole?: () => void;
}) {
    const [menu, setMenu] = useState<{ x: number; y: number; items: ContextMenuItem[] } | null>(null);
    const nameOf = (profile: string) => profiles.find((each) => each.id === profile)?.name ?? "removed";

    return (
        <div className="db-tabstrip">
            <div className="db-tabs" role="tablist" aria-label="Open tabs">
                {tabs.map((tab) => {
                    const label = tabLabel(tab);
                    const connection = nameOf(tab.profile);
                    return (
                        <div
                            key={tab.id}
                            className={`db-tab${tab.id === active ? " active" : ""}`}
                            onMouseDown={(event) => {
                                if (event.button !== 1) return;
                                event.preventDefault();
                                onClose(tab.id);
                            }}
                            onContextMenu={(event) => {
                                event.preventDefault();
                                setMenu({
                                    x: event.clientX,
                                    y: event.clientY,
                                    items: [
                                        { label: "Close", run: () => onClose(tab.id) },
                                        { label: "Close others", disabled: tabs.length < 2, run: () => onCloseOthers(tab.id) },
                                    ],
                                });
                            }}>
                            <button
                                type="button"
                                role="tab"
                                aria-selected={tab.id === active}
                                aria-label={`${connection} ${label}`}
                                title={tab.kind === "table" ? `${connection} · ${tab.schema}.${tab.name}` : `${connection} · ${label}`}
                                className="db-tab-main"
                                onClick={() => onSelect(tab.id)}>
                                <span className={`db-tab-glyph ${tab.kind}`} aria-hidden="true">
                                    {GLYPH[tab.kind]}
                                </span>
                                <span className="db-tab-connection">{connection}</span>
                                <span className="db-tab-label">{label}</span>
                            </button>
                            <button
                                type="button"
                                className="db-tab-close"
                                aria-label={`Close ${connection} ${label}`}
                                onClick={() => onClose(tab.id)}>
                                <IconClose size={10} />
                            </button>
                        </div>
                    );
                })}
            </div>
            {onNewConsole && (
                <button type="button" className="db-icon-button" title="New console" aria-label="New console" onClick={onNewConsole}>
                    <IconPlus size={13} />
                </button>
            )}
            {menu && <ContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={() => setMenu(null)} />}
        </div>
    );
}
