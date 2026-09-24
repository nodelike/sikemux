import { contrastRatio, readableColor } from "../lib/themeContrast";
import { GHOSTTY_THEMES } from "./ghostty.generated";

const channels = (color: string) => [1, 3, 5].map((at) => parseInt(color.slice(at, at + 2), 16));

function mix(from: string, to: string, amount: number): string {
    const a = channels(from),
        b = channels(to);
    return `#${a
        .map((c, i) =>
            Math.round(c + (b[i] - c) * amount)
                .toString(16)
                .padStart(2, "0"),
        )
        .join("")}`;
}

function alpha(color: string, amount: number): string {
    return `rgba(${channels(color).join(",")},${amount})`;
}

function saturation(color: string): number {
    const [r, g, b] = channels(color).map((c) => c / 255);
    const max = Math.max(r, g, b),
        min = Math.min(r, g, b);
    const light = (max + min) / 2;
    return max === min ? 0 : (max - min) / (1 - Math.abs(2 * light - 1));
}

/** The most colourful of the palette's cool hues that still reads on the background. */
function accentFor(bg: string, fg: string, ansi: readonly string[]): string {
    const candidates = [ansi[4], ansi[12], ansi[5], ansi[13], ansi[6], ansi[14], ansi[3], ansi[11]];
    return candidates.find((color) => saturation(color) >= 0.35 && contrastRatio(color, bg) >= 3) ?? fg;
}

export function slugify(name: string): string {
    return name
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "-")
        .replace(/^-|-$/g, "");
}

export interface ThemeColours {
    id: string;
    name: string;
    dark: boolean;
    chrome: string[];
    editor: string[];
    highlight: string[];
    terminal: string[];
}

export function ghosttyTheme(name: string, encoded: string, accent?: string): ThemeColours {
    const [bg, rawFg, cursor, cursorText, rawSelection, ...ansi] = encoded.split(",").map((hex) => (hex ? `#${hex}` : ""));
    const dark = contrastRatio("#ffffff", bg) >= contrastRatio("#000000", bg);
    const bgDim = mix(bg, "#000000", dark ? 0.22 : 0.035);
    const bgRaised = mix(bg, rawFg, dark ? 0.06 : 0.045);
    const grounds = [bg, bgDim, bgRaised];
    const fg = readableColor(rawFg, grounds, dark ? "#ffffff" : "#000000", 5);
    const acc = accent ?? accentFor(bg, fg, ansi);
    const line = mix(bg, fg, dark ? 0.11 : 0.13);
    const selection = rawSelection && contrastRatio(rawSelection, fg) >= 2 ? rawSelection : mix(bg, acc, 0.28);
    const inkDim = mix(fg, bg, 0.38);
    const comment = readableColor(ansi[8], [bg], fg, 3);
    const tone = (color: string) => readableColor(color, [bg], fg, 3);

    const chrome = [bg, bgDim, bgRaised, fg, inkDim, mix(fg, bg, 0.5), acc, alpha(acc, 0.3), alpha(acc, 0.1), line, alpha(acc, 0.055), ansi[1]];
    const editor = [fg, "transparent", cursor, selection, alpha(acc, 0.055), mix(fg, bg, 0.62), acc, line, mix(bg, fg, 0.2)];
    const highlight = [
        tone(ansi[5]),
        tone(ansi[2]),
        comment,
        tone(ansi[3]),
        tone(ansi[4]),
        tone(ansi[11]),
        fg,
        tone(ansi[6]),
        tone(ansi[1]),
        inkDim,
        tone(ansi[6]),
        tone(ansi[9]),
        inkDim,
        acc,
    ];
    const terminal = [bg, rawFg, cursor, cursorText, selection, ...ansi];
    return { id: `ghostty-${slugify(name)}`, name, dark, chrome, editor, highlight, terminal };
}

/** Ghostty's catalogue, minus any theme whose name matches one in `skipNames` once spaces and punctuation are ignored. */
export function ghosttyThemes(skipNames: ReadonlySet<string>): ThemeColours[] {
    return GHOSTTY_THEMES.filter(([name]) => !skipNames.has(slugify(name).replace(/-/g, ""))).map(([name, encoded]) => ghosttyTheme(name, encoded));
}
