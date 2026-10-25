export interface AgentBuddy {
    /** A slot in the pack's palette, given out so open agents rarely share one. */
    color: number;
    pack?: string;
    shape?: string;
}

export type BuddyState = "idle" | "working" | "waiting" | "done" | "error";

export interface BuddyShape {
    id: string;
    name: string;
    /** Markup in a 24×24 box. `uid` keeps gradient and clip ids apart when the same shape is drawn twice. */
    draw: (color: string, state: BuddyState, uid: string) => string;
}

export interface BuddyPack {
    id: string;
    name: string;
    blurb: string;
    colors: readonly string[];
    shapes: readonly BuddyShape[];
}

export const INK = "#1b1622";
export const PASTEL = ["#ffb08a", "#8fd2ff", "#c9ef7d", "#ffe07a", "#c7a9ff", "#ff8d87", "#90efbf", "#ff9fcd", "#80e6de", "#f3ece0"] as const;
export const NEON = ["#5ef2ff", "#ff6fb5", "#ffd23a", "#7dff9a", "#b48cff", "#ff8a4c", "#7ab8ff", "#ff5c7a"] as const;

const channels = (hex: string) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
const toHex = (rgb: number[]) =>
    "#" +
    rgb
        .map((v) =>
            Math.round(Math.max(0, Math.min(255, v)))
                .toString(16)
                .padStart(2, "0"),
        )
        .join("");
export const lighten = (hex: string, t: number) => toHex(channels(hex).map((v) => v + (255 - v) * t));
export const darken = (hex: string, t: number) => toHex(channels(hex).map((v) => v * (1 - t)));

export const stroke = (width = 1, color = INK) => `stroke="${color}" stroke-width="${width}" stroke-linejoin="round" stroke-linecap="round"`;

export const twinkle = (x: number, y: number, color = "#ffe27a") =>
    `<path class="bd-twinkle" d="m${x} ${y - 1.4}.45 1 1 .4-1 .45-.45 1-.45-1-1-.45 1-.4Z" fill="${color}"/>`;

export interface BlobFace {
    ey: number;
    ex: readonly [number, number];
    my: number;
    ck: number;
    ckx?: number;
}

/** The soft face the blob packs share: shiny eyes, blush and a small mouth. */
export function blobFace(f: BlobFace, state: BuddyState, blush = "#ff5f8f"): string {
    const [l, r] = f.ex;
    const y = f.ey;
    const m = f.my;
    const d = f.ckx ?? 2.5;
    const eye = (x: number) =>
        `<ellipse cx="${x}" cy="${y}" rx="1.5" ry="1.95" fill="${INK}"/><circle cx="${x + 0.5}" cy="${y - 0.75}" r=".6" fill="#fff"/>`;
    const cheeks = (opacity = 0.5, rx = 1.4) =>
        `<ellipse cx="${l - d}" cy="${f.ck}" rx="${rx}" ry=".85" fill="${blush}" opacity="${opacity}"/><ellipse cx="${r + d}" cy="${f.ck}" rx="${rx}" ry=".85" fill="${blush}" opacity="${opacity}"/>`;
    const line = `stroke="${INK}" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" fill="none"`;
    switch (state) {
        case "idle":
            return `${cheeks()}<g class="bd-blink">${eye(l)}${eye(r)}</g><path d="M11 ${m - 0.4}q1 1 2 0" ${line}/>`;
        case "working":
            return `${cheeks()}<g class="bd-look">${eye(l)}${eye(r)}</g><ellipse cx="12" cy="${m}" rx=".7" ry=".6" fill="${INK}"/>`;
        case "waiting":
            return `${cheeks()}${[l, r]
                .map(
                    (x) =>
                        `<circle cx="${x}" cy="${y}" r="2.05" fill="${INK}"/><circle cx="${x + 0.6}" cy="${y - 0.8}" r=".75" fill="#fff"/><circle cx="${x - 0.6}" cy="${y + 0.7}" r=".35" fill="#fff"/>`,
                )
                .join("")}<ellipse cx="12" cy="${m}" rx=".9" ry="1" fill="${INK}"/>`;
        case "done":
            return `${cheeks(0.8, 1.6)}${[l, r].map((x) => `<path d="M${x - 1.5} ${y + 0.6}q1.5-2.2 3 0" ${line} stroke-width="1.3"/>`).join("")}<path d="M10.6 ${m - 0.8}q1.4 2 2.8 0Z" fill="${INK}" ${line}/>`;
        case "error":
            return `${cheeks()}<path d="M${l - 1.3} ${y - 1.1}l2.3 1.1-2.3 1.1M${r + 1.3} ${y - 1.1}l-2.3 1.1 2.3 1.1" ${line}/><path d="M10.4 ${m}q.8-.8 1.6 0t1.6 0" ${line} stroke-width="1"/>`;
    }
}

export interface EyeballOptions {
    /** Pupils sit high, as if looking up over an edge. */
    up?: boolean;
}

/** A white eyeball with an iris and pupil. Without an iris colour the pupil is drawn bare. */
export function eyeball(x: number, y: number, r: number, iris: string | null, state: BuddyState, options: EyeballOptions = {}): string {
    const ball = `<circle cx="${x}" cy="${y}" r="${r}" fill="#fff" stroke="${INK}" stroke-width="${Math.max(0.8, r * 0.2)}"/>`;
    if (state === "done") {
        return `<path d="M${x - r * 0.7} ${y + r * 0.25}Q${x} ${y - r * 0.75} ${x + r * 0.7} ${y + r * 0.25}" stroke="${INK}" stroke-width="${r * 0.32}" fill="none" stroke-linecap="round"/>`;
    }
    if (state === "error") {
        return `${ball}<path d="M${x - r * 0.4} ${y - r * 0.4}l${r * 0.8} ${r * 0.8}M${x + r * 0.4} ${y - r * 0.4}l-${r * 0.8} ${r * 0.8}" stroke="${INK}" stroke-width="${r * 0.25}" stroke-linecap="round"/>`;
    }
    const wide = state === "waiting";
    const irisR = r * (wide ? 0.66 : 0.56);
    const pupilR = r * (wide ? 0.42 : 0.32);
    const motion = state === "working" ? "bd-dart" : state === "idle" ? "bd-roam" : "";
    const cy = y + (options.up ? -r * 0.25 : 0);
    const core =
        (iris ? `<circle cx="${x}" cy="${cy}" r="${irisR}" fill="${iris}"/>` : "") +
        `<circle cx="${x}" cy="${cy}" r="${pupilR}" fill="${INK}"/>` +
        `<circle cx="${x - pupilR * 0.6}" cy="${cy - pupilR * 0.6}" r="${Math.max(0.35, pupilR * 0.45)}" fill="#fff"/>` +
        (wide ? `<circle cx="${x + pupilR * 0.5}" cy="${cy + pupilR * 0.6}" r="${pupilR * 0.22}" fill="#fff"/>` : "");
    return `<g class="${state === "idle" ? "bd-blink" : ""}">${ball}<g class="${motion}">${core}</g></g>`;
}
