import { useEffect, useMemo, useRef, useState } from "react";
import { useModalFocus } from "../hooks/useModalFocus";
import { useMouseActive } from "../hooks/useMouseActive";
import { rankBy } from "@shared/lib/fuzzy";
import { leavingOverlay } from "../lib/motion";
import * as cmd from "../state/commands";
import { DEFAULT_THEME_ID, type ThemeSummary } from "../themes";
import { useThemeSummaries } from "../themes/summaries";
import { usePreference } from "../state/preferences";
import { IconSearch } from "@shared/ui/Icons";

/** Every theme by name. Moving through the list wears each one; Enter keeps it and Escape puts the old one back. */
export function ThemePalette() {
    const keptId = usePreference("themeId") ?? DEFAULT_THEME_ID;
    const themes = useThemeSummaries();
    const [query, setQuery] = useState("");
    const entries = useMemo(() => rankBy(query, themes, (theme) => theme.name), [query, themes]);
    const [selected, setSelected] = useState(0);
    // The list arrives from the core after the palette opens; start on the theme kept.
    useEffect(() => {
        setSelected(
            Math.max(
                0,
                themes.findIndex((theme) => theme.id === keptId),
            ),
        );
        // eslint-disable-next-line react-hooks/exhaustive-deps -- once the list arrives, not when the kept theme changes
    }, [themes]);
    const modalRef = useRef<HTMLDivElement>(null);
    useModalFocus(modalRef);
    const inputRef = useRef<HTMLInputElement>(null);
    const listRef = useRef<HTMLDivElement>(null);
    const mouseActive = useMouseActive();
    const previewing = useRef(false);
    const frame = useRef(0);

    useEffect(() => inputRef.current?.focus(), []);
    // Typing wears the best match.
    useEffect(() => {
        if (query.trim()) preview(entries[0]);
        // eslint-disable-next-line react-hooks/exhaustive-deps -- runs when the matches change, not when the query's text does
    }, [entries]);
    useEffect(() => () => cancelAnimationFrame(frame.current), []);

    useEffect(() => {
        const row = listRef.current?.querySelector<HTMLElement>(`.theme-palette-item:nth-child(${selected + 1})`);
        row?.scrollIntoView?.({ block: "nearest" });
    }, [selected, entries]);

    // Held arrow keys move faster than a theme paints, so only the row the keys stopped on is worn.
    const preview = (theme: ThemeSummary | undefined) => {
        if (!theme) return;
        previewing.current = true;
        cancelAnimationFrame(frame.current);
        frame.current = requestAnimationFrame(() => cmd.previewThemeById(theme.id));
    };

    const move = (index: number) => {
        setSelected(index);
        preview(entries[index]);
    };

    const close = () => {
        cancelAnimationFrame(frame.current);
        if (previewing.current) cmd.cancelThemePreview();
        cmd.closeThemePalette();
    };

    const keep = (theme: ThemeSummary | undefined) => {
        if (!theme) return;
        cancelAnimationFrame(frame.current);
        cmd.setThemeId(theme.id);
        cmd.closeThemePalette();
    };

    const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
        const count = entries.length;
        if (event.key === "Escape") {
            event.preventDefault();
            close();
        } else if (event.key === "ArrowDown" || (event.key === "Tab" && !event.shiftKey)) {
            event.preventDefault();
            if (count) move((selected + 1) % count);
        } else if (event.key === "ArrowUp" || (event.key === "Tab" && event.shiftKey)) {
            event.preventDefault();
            if (count) move((selected - 1 + count) % count);
        } else if (event.key === "PageDown" || event.key === "PageUp") {
            event.preventDefault();
            if (count) move(Math.max(0, Math.min(count - 1, selected + (event.key === "PageDown" ? 10 : -10))));
        } else if (event.key === "Enter") {
            event.preventDefault();
            keep(entries[selected]);
        }
    };

    return (
        <div ref={leavingOverlay} className="picker-backdrop theme-palette-backdrop" onMouseDown={close}>
            <div
                ref={modalRef}
                tabIndex={-1}
                className="picker command-palette"
                role="dialog"
                aria-modal="true"
                aria-label="Switch theme"
                onMouseDown={(event) => event.stopPropagation()}>
                <div className="picker-input-wrap">
                    <IconSearch size={15} className="picker-search-icon" />
                    <input
                        ref={inputRef}
                        className="picker-input"
                        role="combobox"
                        aria-expanded="true"
                        aria-controls="theme-results"
                        aria-activedescendant={entries[selected] ? `theme-result-${selected}` : undefined}
                        aria-autocomplete="list"
                        aria-label="Search themes"
                        placeholder="select a theme…"
                        value={query}
                        onChange={(event) => {
                            setQuery(event.target.value);
                            setSelected(0);
                        }}
                        onKeyDown={onKeyDown}
                        spellCheck={false}
                    />
                </div>
                <div className="picker-list command-palette-list" ref={listRef} id="theme-results" role="listbox" aria-label="Themes">
                    {entries.length === 0 && <div className="picker-empty">no themes match</div>}
                    {entries.map((theme, index) => (
                        <button
                            key={theme.id}
                            type="button"
                            role="option"
                            id={`theme-result-${index}`}
                            tabIndex={-1}
                            aria-selected={index === selected}
                            className={`picker-item theme-palette-item${index === selected ? " sel" : ""}`}
                            onMouseEnter={() => {
                                if (mouseActive.current) setSelected(index);
                            }}
                            onClick={() => keep(theme)}>
                            <span className="picker-name">{theme.name}</span>
                            {theme.id === keptId && <span className="command-palette-meta">current</span>}
                            <span className="command-palette-meta">{theme.dark ? "dark" : "light"}</span>
                        </button>
                    ))}
                </div>
            </div>
        </div>
    );
}
