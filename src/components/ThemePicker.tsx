import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { CURATED_THEMES, THEMES, type Theme } from "../themes";
import * as cmd from "../state/commands";
import { IconCheck, IconImage, IconPencil, IconSearch, IconTrash } from "./Icons";

type Tone = "all" | "dark" | "light";

interface Row {
    theme: Theme;
    custom: boolean;
}

interface Group {
    label: string;
    rows: Row[];
}

type Item = { kind: "group"; label: string; count: number; top: number } | { kind: "row"; row: Row; top: number };

const ROW_HEIGHT = 30;
const GROUP_HEIGHT = 28;
const OVERSCAN = 8 * ROW_HEIGHT;
/** jsdom measures nothing, so a list with no measured height renders a screenful. */
const FALLBACK_VIEWPORT = 340;

const CURATED_IDS = new Set(CURATED_THEMES.map((theme) => theme.id));
const GHOSTTY_THEMES = THEMES.filter((theme) => !CURATED_IDS.has(theme.id));

function Swatches({ theme }: { theme: Theme }) {
    const t = theme.terminal;
    return (
        <span className="theme-pick-swatches" aria-hidden="true">
            {[t.red, t.green, t.yellow, t.blue, t.magenta, t.cyan].map((color, i) => (
                <span key={i} style={{ background: color }} />
            ))}
        </span>
    );
}

function Chip({ theme }: { theme: Theme }) {
    return (
        <span className="theme-pick-chip" style={{ background: theme.chrome.bg, borderColor: theme.chrome.line }} aria-hidden="true">
            <span style={{ color: theme.chrome.acc }}>Aa</span>
        </span>
    );
}

export function ThemePicker({
    themeId,
    customThemes,
    editingId,
    onCustomize,
    onEdit,
    onFromWallpaper,
    readingWallpaper,
}: {
    themeId: string;
    customThemes: readonly Theme[];
    editingId?: string;
    onCustomize: (theme: Theme) => void;
    onEdit: (theme: Theme) => void;
    onFromWallpaper: () => void;
    readingWallpaper: boolean;
}) {
    const [query, setQuery] = useState("");
    const [tone, setTone] = useState<Tone>("all");
    const listRef = useRef<HTMLDivElement>(null);

    const groups = useMemo<Group[]>(() => {
        const needle = query.trim().toLowerCase();
        const keep = (theme: Theme) => (tone === "all" || theme.dark === (tone === "dark")) && (!needle || theme.name.toLowerCase().includes(needle));
        const rows = (themes: readonly Theme[], custom: boolean) => themes.filter(keep).map((theme) => ({ theme, custom }));
        return [
            { label: "Your themes", rows: rows(customThemes, true) },
            { label: "Sikemux", rows: rows(CURATED_THEMES, false) },
            { label: "Ghostty", rows: rows(GHOSTTY_THEMES, false) },
        ].filter((group) => group.rows.length > 0);
    }, [customThemes, query, tone]);

    const flat = useMemo(() => groups.flatMap((group) => group.rows), [groups]);
    const layout = useMemo(() => {
        const items: Item[] = [];
        let top = 0;
        for (const group of groups) {
            items.push({ kind: "group", label: group.label, count: group.rows.length, top });
            top += GROUP_HEIGHT;
            for (const row of group.rows) {
                items.push({ kind: "row", row, top });
                top += ROW_HEIGHT;
            }
        }
        return { items, height: top };
    }, [groups]);
    const [scrollTop, setScrollTop] = useState(0);
    const [viewport, setViewport] = useState(FALLBACK_VIEWPORT);
    useLayoutEffect(() => {
        const height = listRef.current?.clientHeight;
        if (height) setViewport(height);
    }, [layout]);
    const visible = layout.items.filter((item) => item.top + ROW_HEIGHT >= scrollTop - OVERSCAN && item.top <= scrollTop + viewport + OVERSCAN);
    const stuck = layout.items.filter((item) => item.kind === "group" && item.top <= scrollTop).at(-1);
    const current = [...customThemes, ...THEMES].find((theme) => theme.id === themeId) ?? THEMES[0];
    const currentIsCustom = customThemes.some((theme) => theme.id === current.id);

    useEffect(() => {
        const list = listRef.current;
        if (!list) return;
        const item = layout.items.find((candidate) => candidate.kind === "row" && candidate.row.theme.id === themeId);
        const height = list.clientHeight || FALLBACK_VIEWPORT;
        let next = item ? list.scrollTop : 0;
        if (item && item.top - GROUP_HEIGHT < next) next = Math.max(0, item.top - GROUP_HEIGHT);
        else if (item && item.top + ROW_HEIGHT > next + height) next = item.top + ROW_HEIGHT - height;
        list.scrollTop = next;
        setScrollTop(next);
    }, [layout, themeId]);

    const step = (delta: number) => {
        if (flat.length === 0) return;
        const at = flat.findIndex((row) => row.theme.id === themeId);
        const next = at < 0 ? (delta > 0 ? 0 : flat.length - 1) : Math.max(0, Math.min(flat.length - 1, at + delta));
        cmd.setThemeId(flat[next].theme.id);
    };

    return (
        <div className="theme-pick">
            <div className="theme-pick-current">
                <div className="settings-theme-preview theme-pick-preview" style={{ background: current.chrome.bg, color: current.chrome.ink }}>
                    <span className="settings-theme-preview-mark" style={{ color: current.chrome.acc }}>
                        Aa
                    </span>
                    <span className="settings-theme-preview-code" style={{ color: current.highlight.comment }}>
                        // make it yours
                    </span>
                </div>
                <div className="theme-pick-current-body">
                    <span className="theme-pick-current-name">{current.name}</span>
                    <Swatches theme={current} />
                </div>
                <button
                    className="settings-btn"
                    onClick={onFromWallpaper}
                    disabled={readingWallpaper}
                    title="Build a theme from the colours of your desktop wallpaper"
                    type="button">
                    <IconImage size={12} /> {readingWallpaper ? "Reading…" : "From wallpaper"}
                </button>
                <button
                    className="settings-btn"
                    onClick={() => (currentIsCustom ? onEdit(current) : onCustomize(current))}
                    title={currentIsCustom ? "Edit this theme" : "Fork this theme into an editable copy"}
                    type="button">
                    <IconPencil size={11} /> {currentIsCustom ? "Edit" : "Customize"}
                </button>
            </div>

            <div className="theme-pick-bar">
                <label className="theme-pick-search">
                    <IconSearch size={12} />
                    <input
                        value={query}
                        onChange={(event) => setQuery(event.target.value)}
                        onKeyDown={(event) => {
                            if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                                event.preventDefault();
                                step(event.key === "ArrowDown" ? 1 : -1);
                            } else if (event.key === "Escape" && query) {
                                event.preventDefault();
                                event.stopPropagation();
                                setQuery("");
                            }
                        }}
                        placeholder={`Search ${THEMES.length + customThemes.length} themes`}
                        aria-label="Search themes"
                        spellCheck={false}
                    />
                </label>
                <div className="theme-pick-tones" role="radiogroup" aria-label="Filter by appearance">
                    {(["all", "dark", "light"] as const).map((option) => (
                        <button
                            key={option}
                            type="button"
                            role="radio"
                            aria-checked={tone === option}
                            className={`theme-pick-tone${tone === option ? " on" : ""}`}
                            onClick={() => setTone(option)}>
                            {option}
                        </button>
                    ))}
                </div>
            </div>

            <div
                className="theme-pick-list"
                ref={listRef}
                role="listbox"
                aria-label="Themes"
                onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}>
                {stuck?.kind === "group" && (
                    <div className="theme-pick-group theme-pick-stuck" aria-hidden="true">
                        {stuck.label}
                        <span>{stuck.count}</span>
                    </div>
                )}
                <div className="theme-pick-track" style={{ height: layout.height }}>
                    {visible.map((item) => {
                        if (item.kind === "group")
                            return (
                                <div key={`group-${item.label}`} className="theme-pick-group" style={{ top: item.top }}>
                                    {item.label}
                                    <span>{item.count}</span>
                                </div>
                            );
                        const { theme, custom } = item.row;
                        const active = theme.id === themeId;
                        return (
                            <div
                                key={theme.id}
                                role="option"
                                aria-selected={active}
                                className={`theme-pick-row${active ? " active" : ""}${theme.id === editingId ? " editing" : ""}`}
                                style={{ top: item.top }}
                                onClick={() => cmd.setThemeId(theme.id)}>
                                <Chip theme={theme} />
                                <span className="theme-pick-name">{theme.name}</span>
                                <span className="theme-pick-actions">
                                    <button
                                        className="settings-theme-act"
                                        onClick={(event) => {
                                            event.stopPropagation();
                                            if (custom) onEdit(theme);
                                            else onCustomize(theme);
                                        }}
                                        title={custom ? "Edit theme" : "Customize a copy"}
                                        type="button">
                                        <IconPencil size={11} />
                                    </button>
                                    {custom && (
                                        <button
                                            className="settings-theme-act danger"
                                            onClick={(event) => {
                                                event.stopPropagation();
                                                cmd.deleteCustomTheme(theme.id);
                                            }}
                                            title="Delete theme"
                                            type="button">
                                            <IconTrash size={11} />
                                        </button>
                                    )}
                                </span>
                                <Swatches theme={theme} />
                                <span className="theme-pick-check">{active && <IconCheck size={11} />}</span>
                            </div>
                        );
                    })}
                </div>
                {groups.length === 0 && <div className="theme-pick-empty">No theme matches “{query}”.</div>}
            </div>
        </div>
    );
}
