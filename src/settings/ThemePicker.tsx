import { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import type { Theme, ThemeSummary } from "../themes";
import { currentTheme, subscribeTheme } from "../themes/bus";
import * as cmd from "../state/commands";
import { IconCheck, IconImage, IconPencil, IconSearch, IconTrash } from "@shared/ui/Icons";

type Tone = "all" | "dark" | "light";

interface Row {
    theme: ThemeSummary;
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

const GROUPS: readonly [ThemeSummary["source"], string][] = [
    ["custom", "Your themes"],
    ["sikemux", "Sikemux"],
    ["ghostty", "Ghostty"],
];

function Swatches({ colours }: { colours: readonly string[] }) {
    return (
        <span className="theme-pick-swatches" aria-hidden="true">
            {colours.map((color, i) => (
                <span key={i} style={{ background: color }} />
            ))}
        </span>
    );
}

function Chip({ theme }: { theme: ThemeSummary }) {
    return (
        <span className="theme-pick-chip" style={{ background: theme.bg, borderColor: theme.line }} aria-hidden="true">
            <span style={{ color: theme.acc }}>Aa</span>
        </span>
    );
}

export type PictureSource = "wallpaper" | "paneImage";

export function ThemePicker({
    summaries,
    themeId,
    customThemes,
    editingId,
    onCustomize,
    onEdit,
    onFromWallpaper,
    onFromPaneImage,
    readingPicture,
}: {
    summaries: readonly ThemeSummary[];
    themeId: string;
    customThemes: readonly Theme[];
    editingId?: string;
    onCustomize: (id: string) => void;
    onEdit: (theme: Theme) => void;
    onFromWallpaper: () => void;
    onFromPaneImage?: () => void;
    readingPicture: PictureSource | null;
}) {
    const [query, setQuery] = useState("");
    const [tone, setTone] = useState<Tone>("all");
    const listRef = useRef<HTMLDivElement>(null);

    const current = useSyncExternalStore(subscribeTheme, currentTheme);
    const groups = useMemo<Group[]>(() => {
        const needle = query.trim().toLowerCase();
        const keep = (theme: ThemeSummary) =>
            (tone === "all" || theme.dark === (tone === "dark")) && (!needle || theme.name.toLowerCase().includes(needle));
        return GROUPS.map(([source, label]) => ({
            label,
            rows: summaries.filter((theme) => theme.source === source && keep(theme)).map((theme) => ({ theme, custom: source === "custom" })),
        })).filter((group) => group.rows.length > 0);
    }, [summaries, query, tone]);

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
    const currentIsCustom = customThemes.some((theme) => theme.id === current.id);
    const edit = (id: string) => {
        const theme = customThemes.find((candidate) => candidate.id === id);
        if (theme) onEdit(theme);
    };

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
                    <Swatches
                        colours={[
                            current.terminal.red,
                            current.terminal.green,
                            current.terminal.yellow,
                            current.terminal.blue,
                            current.terminal.magenta,
                            current.terminal.cyan,
                        ]}
                    />
                </div>
                <button
                    className="settings-btn"
                    onClick={onFromWallpaper}
                    disabled={readingPicture !== null}
                    title="Build a theme from the colours of your desktop wallpaper"
                    type="button">
                    <IconImage size={12} /> {readingPicture === "wallpaper" ? "Reading…" : "From wallpaper"}
                </button>
                {onFromPaneImage && (
                    <button
                        className="settings-btn"
                        onClick={onFromPaneImage}
                        disabled={readingPicture !== null}
                        title="Build a theme from the colours of your pane image"
                        type="button">
                        <IconImage size={12} /> {readingPicture === "paneImage" ? "Reading…" : "From pane image"}
                    </button>
                )}
                <button
                    className="settings-btn"
                    onClick={() => (currentIsCustom ? onEdit(current) : onCustomize(current.id))}
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
                        placeholder={`Search ${summaries.length} themes`}
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
                                            if (custom) edit(theme.id);
                                            else onCustomize(theme.id);
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
                                <Swatches colours={theme.swatches} />
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
