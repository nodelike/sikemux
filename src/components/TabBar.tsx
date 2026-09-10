import { navigateTabs } from "../lib/tabNavigation";
import { useState } from "react";
import type { CSSProperties, ReactNode } from "react";
import { TreeContextMenu, type CtxItem } from "./FileTree";
import { IconClose } from "./Icons";
import { Tooltip } from "./Tooltip";

/**
 * One normalized tab. Every tab strip in the app (editor files, agents,
 * terminals, Bruno requests) describes its tabs as these, so selection,
 * closing, the dirty dot, accessories and the right-click menu all behave
 * identically. A new group only has to map its state into `TabDescriptor[]`.
 */
export interface TabDescriptor {
    /** Stable identity — used as the React key and passed to onSelect/onClose. */
    id: string;
    label: string;
    tabId?: string;
    panelId?: string;
    /** Leading glyph/badge rendered before the label (FileIcon, agent glyph, method badge…). */
    icon?: ReactNode;
    /** Show the unsaved-changes dot. */
    dirty?: boolean;
    active?: boolean;
    /** Defaults to whether `onClose` is provided; set false to pin a tab open. */
    closable?: boolean;
    title?: string;
    /** Extra control rendered just before the close button (e.g. a per-tab status badge). */
    accessory?: ReactNode;
}

export type TabVariant = "editor" | "agent" | "bruno";

interface TabBarProps {
    variant: TabVariant;
    tabs: TabDescriptor[];
    onSelect: (id: string) => void;
    onClose?: (id: string) => void;
    /** Build the right-click menu for a tab. Omit to disable the context menu. */
    buildMenu?: (id: string) => CtxItem[];
    onAdd?: () => void;
    addIcon?: ReactNode;
    addTitle?: string;
    trailing?: ReactNode;
    style?: CSSProperties;
}

export function TabBar({ variant, tabs, onSelect, onClose, buildMenu, onAdd, addIcon, addTitle, trailing, style }: TabBarProps) {
    const [menu, setMenu] = useState<{ x: number; y: number; id: string } | null>(null);
    const menuItems = menu && buildMenu ? buildMenu(menu.id) : null;

    return (
        <div className={`tabbar v-${variant}`} style={style} role="tablist">
            {tabs.map((t) => {
                const closable = t.closable ?? !!onClose;
                return (
                    <div key={t.id} className={`tab-wrap${t.active ? " active" : ""}`} role="presentation">
                        <Tooltip label={t.title}>
                            <button
                                type="button"
                                role="tab"
                                id={t.tabId}
                                aria-controls={t.panelId}
                                aria-selected={t.active ?? false}
                                tabIndex={t.active || (!tabs.some((tab) => tab.active) && tabs[0] === t) ? 0 : -1}
                                onKeyDown={(event) => {
                                    navigateTabs(event);
                                    if (event.key === "Delete" && closable && onClose) {
                                        event.preventDefault();
                                        const buttons = [
                                            ...(event.currentTarget
                                                .closest('[role="tablist"]')
                                                ?.querySelectorAll<HTMLButtonElement>('[role="tab"]') ?? []),
                                        ];
                                        const index = tabs.indexOf(t);
                                        (buttons[index + 1] ?? buttons[index - 1])?.focus();
                                        onClose(t.id);
                                    }
                                    if (event.shiftKey && event.key === "F10" && buildMenu) {
                                        event.preventDefault();
                                        const rect = event.currentTarget.getBoundingClientRect();
                                        setMenu({ x: rect.left, y: rect.bottom, id: t.id });
                                    }
                                }}
                                aria-label={`${t.label}${t.dirty ? ", unsaved changes" : ""}`}
                                className={`tab${t.active ? " active" : ""}`}
                                onClick={(event) => {
                                    event.currentTarget.focus();
                                    onSelect(t.id);
                                }}
                                onContextMenu={
                                    buildMenu
                                        ? (e) => {
                                              e.preventDefault();
                                              setMenu({ x: e.clientX, y: e.clientY, id: t.id });
                                          }
                                        : undefined
                                }>
                                {t.icon}
                                <span className="tab-label">{t.label}</span>
                                {t.dirty && <span className="tab-dot" aria-hidden="true" />}
                                {t.accessory}
                            </button>
                        </Tooltip>
                        {closable && onClose && (
                            <Tooltip label={`Close ${t.label}`}>
                                <button type="button" className="tab-x" aria-label={`Close ${t.label}`} onClick={() => onClose(t.id)}>
                                    <IconClose size={11} />
                                </button>
                            </Tooltip>
                        )}
                    </div>
                );
            })}
            {onAdd && (
                <Tooltip label={addTitle}>
                    <button type="button" className="tab-add" aria-label={addTitle} onClick={onAdd}>
                        {addIcon}
                    </button>
                </Tooltip>
            )}
            {trailing && <div className="tabbar-trailing">{trailing}</div>}
            {menu && menuItems && <TreeContextMenu x={menu.x} y={menu.y} items={menuItems} onClose={() => setMenu(null)} />}
        </div>
    );
}
