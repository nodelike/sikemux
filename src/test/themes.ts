import curated from "../../src-tauri/crates/sikemux-core/src/server/themes/curated.json";
import type { Palette, Theme } from "../themes";

/** The Sikemux themes as the core keeps them, for tests that need real themes. */
export const SIKEMUX_THEMES = curated as Theme[];

export function themeById(id: string): Theme {
    const theme = SIKEMUX_THEMES.find((candidate) => candidate.id === id);
    if (!theme) throw new Error(`no Sikemux theme ${id}`);
    return theme;
}

/** A theme as the core would hand it over, its tokens all its ink: enough for tests that switch themes. */
export function testPalette(theme: Theme): Palette {
    return {
        theme,
        tokens: {
            sunken: theme.chrome.ink,
            ground: theme.chrome.ink,
            raised: theme.chrome.ink,
            overlay: theme.chrome.ink,
            composer: theme.chrome.ink,
            surfaceComposer: theme.chrome.ink,
            hover: theme.chrome.ink,
            chosen: theme.chrome.ink,
            active: theme.chrome.ink,
            selected: theme.chrome.ink,
            tooltip: theme.chrome.ink,
            border: theme.chrome.ink,
            borderStrong: theme.chrome.ink,
            borderSelected: theme.chrome.ink,
            line: theme.chrome.ink,
            lineSoft: theme.chrome.ink,
            ink: theme.chrome.ink,
            secondary: theme.chrome.ink,
            tertiary: theme.chrome.ink,
            disabled: theme.chrome.ink,
            inkDim: theme.chrome.ink,
            inkFaint: theme.chrome.ink,
            rest: theme.chrome.ink,
            iconQuiet: theme.chrome.ink,
            accent: theme.chrome.ink,
            accentSoft: theme.chrome.ink,
            onAccent: theme.chrome.ink,
            live: theme.chrome.ink,
            warn: theme.chrome.ink,
            danger: theme.chrome.ink,
            cmd: theme.chrome.ink,
            host: theme.chrome.ink,
            ssh: theme.chrome.ink,
            treeSpine: theme.chrome.ink,
            treeTick: theme.chrome.ink,
            gitAdded: theme.chrome.ink,
            gitModified: theme.chrome.ink,
            gitUntracked: theme.chrome.ink,
            gitDeleted: theme.chrome.ink,
            gitRenamed: theme.chrome.ink,
            toolRead: theme.chrome.ink,
            toolEdit: theme.chrome.ink,
            toolDelete: theme.chrome.ink,
            toolRun: theme.chrome.ink,
            toolMcp: theme.chrome.ink,
            shaderDot: theme.chrome.ink,
        },
    };
}
