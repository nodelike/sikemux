/**
 * The phone's colour names and the theme tokens each one is read from. The
 * phone's composer sits one step above its ground, where the Mac's sits on its
 * darkest surface, so that one is mixed here rather than read.
 */
export const PALETTE_TOKENS: Readonly<Record<string, string>> = {
    sunken: "var(--surface-sunken)",
    ground: "var(--surface)",
    raised: "var(--surface-raised)",
    overlay: "var(--surface-overlay)",
    composer: "color-mix(in oklab, var(--ink) 5%, var(--surface))",
    border: "var(--border)",
    borderStrong: "var(--border-strong)",
    rest: "var(--gray-600)",
    ink: "var(--text-primary)",
    secondary: "var(--text-secondary)",
    tertiary: "var(--text-tertiary)",
    inkDim: "var(--ink-dim)",
    inkFaint: "var(--ink-faint)",
    active: "var(--surface-active)",
    selected: "var(--surface-selected)",
    borderSelected: "var(--border-selected)",
    accent: "var(--acc)",
    accentSoft: "color-mix(in oklab, var(--acc) 10%, transparent)",
    live: "var(--live)",
    warn: "var(--warn)",
    danger: "var(--danger)",
    cmd: "var(--cmd)",
    treeSpine: "var(--tree-spine)",
    treeTick: "var(--tree-tick)",
    gitAdded: "var(--git-added)",
    gitModified: "var(--git-modified)",
    gitDeleted: "var(--git-deleted)",
    gitRenamed: "var(--git-renamed)",
    toolRead: "color-mix(in oklab, var(--git-renamed) 76%, var(--ink-dim))",
    toolEdit: "color-mix(in oklab, var(--git-modified) 76%, var(--ink-dim))",
    toolDelete: "color-mix(in oklab, var(--git-deleted) 76%, var(--ink-dim))",
    toolRun: "color-mix(in oklab, var(--cmd) 70%, var(--ink-dim))",
};

function hex(part: number): string {
    return part.toString(16).padStart(2, "0");
}

/** A colour as a phone can draw it: `#rrggbb`, or `rgba(…)` when it is see-through. */
export function plainColor([red, green, blue, alpha]: ArrayLike<number> & Iterable<number>): string {
    if (alpha === 255) return `#${hex(red)}${hex(green)}${hex(blue)}`;
    return `rgba(${red}, ${green}, ${blue}, ${Math.round((alpha / 255) * 100) / 100})`;
}

/**
 * Reads each token as the window paints it. A computed colour can still be a
 * `color-mix()` or `color(srgb …)` a phone cannot parse, so each is painted
 * into one pixel and read back.
 */
export function readPalette(extra: Readonly<Record<string, string>> = {}, root: HTMLElement = document.documentElement): Record<string, string> {
    const probe = document.createElement("span");
    probe.style.display = "none";
    root.appendChild(probe);
    const canvas = document.createElement("canvas");
    canvas.width = 1;
    canvas.height = 1;
    const paint = canvas.getContext("2d", { willReadFrequently: true });
    const palette: Record<string, string> = {};
    try {
        if (!paint) return palette;
        for (const [name, token] of Object.entries({ ...PALETTE_TOKENS, ...extra })) {
            probe.style.color = "";
            probe.style.color = token;
            const computed = getComputedStyle(probe).color;
            if (!computed) continue;
            paint.clearRect(0, 0, 1, 1);
            paint.fillStyle = computed;
            paint.fillRect(0, 0, 1, 1);
            palette[name] = plainColor(paint.getImageData(0, 0, 1, 1).data);
        }
    } finally {
        probe.remove();
    }
    return palette;
}
