import { darken, stroke, type BuddyPack, type BuddyShape, type BuddyState } from "../shared";

const INK = "#2a1f2e";
const WHITE = "#fffaf2";
const WAFU = ["#e2513a", "#3d5a9e", "#f6a5c0", "#8bb35a", "#f2b830", "#9f86cc", "#33a6b8", "#e98b2a"] as const;

interface DotFace {
    cx: number;
    ey: number;
    sp: number;
    my: number;
    cky: number;
    cks: number;
    r?: number;
}

/** Small dot eyes set wide and low, the way Japanese mascots are drawn. */
function dotFace(f: DotFace, state: BuddyState): string {
    const { cx, ey, sp, my, cky, cks } = f;
    const r = f.r ?? 1;
    const ex = [cx - sp, cx + sp];
    const w = r * 0.8;
    const dot = (x: number, k = 1) =>
        `<circle cx="${x}" cy="${ey}" r="${r * k}" fill="${INK}"/><circle cx="${x + r * 0.35 * k}" cy="${ey - r * 0.4 * k}" r="${r * 0.36 * k}" fill="#fff"/>`;
    const blush = `<ellipse cx="${cx - cks}" cy="${cky}" rx="${r * 1.25}" ry="${r * 0.7}" fill="#ff7a9a" opacity=".6"/><ellipse cx="${cx + cks}" cy="${cky}" rx="${r * 1.25}" ry="${r * 0.7}" fill="#ff7a9a" opacity=".6"/>`;
    const line = `stroke="${INK}" stroke-width="${r * 0.8}" stroke-linecap="round" stroke-linejoin="round" fill="none"`;
    switch (state) {
        case "idle":
            return `${blush}<g class="bd-blink">${dot(ex[0])}${dot(ex[1])}</g><path d="M${cx - w} ${my}q${w / 2} ${w * 0.7} ${w} 0q${w / 2} ${w * 0.7} ${w} 0" ${line}/>`;
        case "working":
            return `${blush}<g class="bd-look">${dot(ex[0])}${dot(ex[1])}</g><path d="M${cx - w * 0.6} ${my + 0.2}h${w * 1.2}" ${line}/>`;
        case "waiting":
            return `${blush}${dot(ex[0], 1.35)}${dot(ex[1], 1.35)}<ellipse cx="${cx}" cy="${my + 0.2}" rx="${w * 0.55}" ry="${w * 0.65}" fill="${INK}"/>`;
        case "done":
            return `${blush}${ex.map((x) => `<path d="M${x - r * 1.2} ${ey + r * 0.5}q${r * 1.2} -${r * 1.7} ${r * 2.4} 0" ${line} stroke-width="${r}"/>`).join("")}<path d="M${cx - w} ${my - 0.2}q${w} ${w * 1.6} ${w * 2} 0Z" fill="#c2405f" ${line}/>`;
        case "error":
            return `${blush}<path d="M${ex[0] - r} ${ey - r}l${r * 1.6} ${r}-${r * 1.6} ${r}M${ex[1] + r} ${ey - r}l-${r * 1.6} ${r} ${r * 1.6} ${r}" ${line} stroke-width="${r * 0.9}"/><path d="M${cx - w} ${my + 0.2}q${w / 2}-${w / 2} ${w} 0t${w} 0" ${line}/>`;
    }
}

const S = (w = 1) => stroke(w, INK);
const sweat = (x: number, y: number) =>
    `<path class="bd-drip" d="M${x} ${y}c.9 1.2 1.3 2 1.3 2.6a1.3 1.3 0 0 1-2.6 0c0-.6.4-1.4 1.3-2.6Z" fill="#8fd2ff" ${S(0.6)}/>`;
const mascot = (id: string, name: string, body: (c: string, s: BuddyState) => string, sweatAt: [number, number]): BuddyShape => ({
    id,
    name,
    draw: (c, s) => body(c, s) + (s === "error" ? sweat(...sweatAt) : ""),
});

export const japan: BuddyPack = {
    id: "japan",
    name: "Japan",
    blurb: "daruma, maneki-neko and friends",
    colors: WAFU,
    shapes: [
        mascot(
            "daruma",
            "Daruma",
            (c, s) =>
                `<g class="${s === "idle" ? "bd-rock" : ""}"><path d="M12 3.4c5 0 8.6 4 8.6 9.8 0 5.4-3.8 8.8-8.6 8.8s-8.6-3.4-8.6-8.8C3.4 7.4 7 3.4 12 3.4Z" fill="${c}" ${S(1.1)}/><ellipse cx="12" cy="11.6" rx="5.8" ry="4.8" fill="${WHITE}" ${S(0.9)}/><path d="M7.6 8.8q1.6-1.7 3.4-.3M16.4 8.8q-1.6-1.7-3.4-.3" stroke="${INK}" stroke-width="1.3" stroke-linecap="round" fill="none"/><path d="M8.2 15q1.3 1 2.6 0M13.2 15q1.3 1 2.6 0" stroke="#9a8f9c" stroke-width=".8" stroke-linecap="round" fill="none"/><path d="M8.6 18.6q3.4 1.6 6.8 0M10 20.2q2 .8 4 0" stroke="#f2c14e" stroke-width="1.1" stroke-linecap="round" fill="none"/>${dotFace({ cx: 12, ey: 11.4, sp: 2.4, my: 13.4, cky: 12.9, cks: 4, r: 1.05 }, s)}</g>`,
            [19.6, 6],
        ),
        mascot(
            "neko",
            "Maneki-neko",
            (c, s) =>
                `<g class="${s === "idle" || s === "waiting" ? "bd-wave" : ""}"><ellipse cx="20.4" cy="9.6" rx="1.9" ry="2.4" fill="${WHITE}" ${S()}/><circle cx="20.4" cy="9.4" r=".7" fill="${c}"/></g><path d="M4.2 10.6 4.8 4.2l4 3c2-.6 4.4-.6 6.4 0l4-3 .6 6.4c.8 1.2 1.2 2.6 1.2 4 0 4.2-4 7.2-9 7.2s-9-3-9-7.2c0-1.4.4-2.8 1.2-4Z" fill="${WHITE}" ${S(1.1)}/><path d="M5.6 6 6 9.2l2.2-1.6ZM18.4 6 18 9.2l-2.2-1.6Z" fill="${c}" opacity=".85"/><ellipse cx="8" cy="8.8" rx="1.8" ry="1.2" fill="#f2a65a" opacity=".9"/><path d="M5.8 18.4q6.2 2.8 12.4 0" stroke="${c}" stroke-width="1.8" stroke-linecap="round" fill="none"/><circle cx="12" cy="20.4" r="1.25" fill="#f2c14e" ${S(0.7)}/><path d="M11.2 20.6h1.6" stroke="${INK}" stroke-width=".5"/><path d="M3.2 14.4l2.2.3M3.4 16l2.1-.3M20.8 14.4l-2.2.3M20.6 16l-2.1-.3" stroke="${INK}" stroke-width=".5" stroke-linecap="round" opacity=".6"/>${dotFace({ cx: 12, ey: 13.4, sp: 2.8, my: 15.6, cky: 15.4, cks: 4.8, r: 1.05 }, s)}`,
            [3, 5.4],
        ),
        mascot(
            "kokeshi",
            "Kokeshi",
            (c, s) =>
                `<path d="M7.8 14.2h8.4l1.4 7c.1.6-.3 1-.9 1H7.3c-.6 0-1-.4-.9-1Z" fill="${c}" ${S()}/><path d="M9.6 14.2l2.4 2.6 2.4-2.6" stroke="${WHITE}" stroke-width="1" fill="none" stroke-linejoin="round"/><circle cx="12" cy="19.4" r="1.3" fill="${WHITE}" opacity=".9"/><circle cx="12" cy="19.4" r=".5" fill="${c}"/><circle cx="12" cy="9.2" r="6.2" fill="#ffeede" ${S()}/><path d="M5.8 10.2V9c0-3.6 2.8-6 6.2-6s6.2 2.4 6.2 6v1.2c-.6-.3-1-.8-1.2-1.6H7c-.2.8-.6 1.3-1.2 1.6Z" fill="#2c2430" ${S()}/><circle cx="16.6" cy="4.6" r="1.2" fill="${c}" ${S(0.6)}/><circle cx="16.6" cy="4.6" r=".4" fill="#ffe27a"/>${dotFace({ cx: 12, ey: 10.9, sp: 2, my: 12.8, cky: 12.4, cks: 3.5, r: 0.95 }, s)}`,
            [19.4, 9],
        ),
        mascot(
            "kitsune",
            "Kitsune",
            (c, s) =>
                `<path d="M3.4 2.8 7.6 6.6c1.4-.6 2.9-.9 4.4-.9s3 .3 4.4.9L20.6 2.8c.9 3 .9 6 .2 8.8.3 4.8-3.4 10.4-8.8 10.4S2.9 16.4 3.2 11.6c-.7-2.8-.7-5.8.2-8.8Z" fill="${WHITE}" ${S(1.1)}/><path d="M4.6 4.8 5 8.8l2.2-1.6ZM19.4 4.8 19 8.8l-2.2-1.6Z" fill="${c}"/><path d="M12 7.4c.9 1 1 2 0 3-1-1-.9-2 0-3Z" fill="${c}"/><path d="M6.2 11.2q2.4-1.4 4.4.4M17.8 11.2q-2.4-1.4-4.4.4" stroke="${c}" stroke-width="1.2" stroke-linecap="round" fill="none"/><path d="M5 16.2h2.2M5.2 17.6h2M19 16.2h-2.2M18.8 17.6h-2" stroke="${c}" stroke-width=".8" stroke-linecap="round"/><ellipse cx="12" cy="17" rx=".9" ry=".6" fill="${INK}"/>${dotFace({ cx: 12, ey: 13.2, sp: 3, my: 18.6, cky: 15.2, cks: 5, r: 1 }, s)}`,
            [20.6, 13],
        ),
        mascot(
            "teru",
            "Teru teru",
            (c, s) =>
                `<g class="${s === "idle" ? "bd-rock" : ""}"><path d="M7.4 14.4C5.8 17.4 4.6 20 4 22.2h16c-.6-2.2-1.8-4.8-3.4-7.8Z" fill="${WHITE}" ${S()}/><circle cx="12" cy="9.4" r="6.4" fill="${WHITE}" ${S(1.1)}/><path d="M7.4 14.8q4.6 1.8 9.2 0" stroke="${c}" stroke-width="1.6" stroke-linecap="round" fill="none"/><path d="M12 15.7 9.8 14.4v2.6ZM12 15.7l2.2-1.3v2.6Z" fill="${c}" ${S(0.6)}/>${dotFace({ cx: 12, ey: 9.4, sp: 2.4, my: 11.6, cky: 11.1, cks: 4.1, r: 1.05 }, s)}</g>`,
            [18.6, 5],
        ),
        mascot(
            "nigiri",
            "Nigiri",
            (c, s) =>
                `<path d="M3.4 15.6c0-2 1.4-3 3-3h11.2c1.6 0 3 1 3 3v3.4c0 1.8-1.4 3-3 3H6.4c-1.6 0-3-1.2-3-3Z" fill="${WHITE}" ${S()}/><path d="M5.4 20.4l.6-.4M18 20.6l.6-.3M19 16.4l.5.4" stroke="#cfc6bc" stroke-width=".6" stroke-linecap="round"/><path d="M2.6 12.6c0-2.6 2.4-4.6 5.4-4.6h8c3 0 5.4 2 5.4 4.6 0 .8-.6 1.4-1.4 1.4H4c-.8 0-1.4-.6-1.4-1.4Z" fill="${c}" ${S()}/><path d="M7.4 8.6l1.4 5M11.4 8.2l1.4 5.6M15.4 8.4l1.4 5.4" stroke="#fff" stroke-width=".9" opacity=".5"/>${dotFace({ cx: 12, ey: 17, sp: 2.4, my: 18.9, cky: 18.5, cks: 4.6, r: 1 }, s)}`,
            [21, 4.6],
        ),
        mascot(
            "onigiri",
            "Onigiri",
            (c, s) =>
                `<path d="M12 4.6c1.5 0 2.5 1 3.4 2.7l5.4 9.2c1.3 2.3 0 5.3-2.9 5.3H6.1c-2.9 0-4.2-3-2.9-5.3l5.4-9.2c.9-1.7 1.9-2.7 3.4-2.7Z" fill="${WHITE}" ${S(1.1)}/><path d="M8.6 17.8h6.8v4H8.6Z" fill="#2c3a2e"/><circle cx="12" cy="8.6" r="1.1" fill="${c}"/>${dotFace({ cx: 12, ey: 13.4, sp: 2.4, my: 15.4, cky: 15, cks: 4.1, r: 1 }, s)}`,
            [18.6, 7],
        ),
    ],
};

const SKIN = "#ffe7da";
const HAIR = ["#ff9ac8", "#8fc4ff", "#e3e6f0", "#b99cff", "#ffab6b", "#7fe0b6", "#ffdc7a", "#6d7ff0"] as const;
const FACE = `<path d="M5 12.2C5 7.8 8 5.6 12 5.6s7 2.2 7 6.6c0 4.8-3.2 8.6-7 8.6s-7-3.8-7-8.6Z" fill="${SKIN}" ${S()}/>`;

function chibiEyes(state: BuddyState, iris: string): string {
    const ex = [9.1, 14.9];
    const y = 13.9;
    const lid = (x: number, k = 1) =>
        `<path d="M${x - 2.1 * k} ${y - 1.7 * k}q${2.1 * k}-${1.6 * k} ${4.2 * k} 0" stroke="${INK}" stroke-width="1.05" fill="none" stroke-linecap="round"/>`;
    const open = (x: number) =>
        `<ellipse cx="${x}" cy="${y}" rx="1.75" ry="2.35" fill="${iris}"/><ellipse cx="${x}" cy="${y + 0.25}" rx=".8" ry="1.1" fill="${INK}"/><circle cx="${x - 0.55}" cy="${y - 0.9}" r=".72" fill="#fff"/><circle cx="${x + 0.6}" cy="${y + 1.1}" r=".35" fill="#fff" opacity=".9"/>${lid(x)}`;
    switch (state) {
        case "idle":
            return `<g class="bd-blink">${open(ex[0])}${open(ex[1])}</g>`;
        case "working":
            return `<g class="bd-look">${open(ex[0])}${open(ex[1])}</g>`;
        case "waiting":
            return ex
                .map(
                    (x) =>
                        `<ellipse cx="${x}" cy="${y}" rx="1.9" ry="2.5" fill="${iris}"/><path d="m${x} ${y - 1.5}.45 1.05 1.05.45-1.05.45L${x} ${y + 0.5}l-.45-1.05-1.05-.45 1.05-.45Z" fill="#fff"/><circle cx="${x + 0.7}" cy="${y + 1.2}" r=".35" fill="#fff"/>${lid(x, 1.05)}`,
                )
                .join("");
        case "done":
            return ex
                .map((x) => `<path d="M${x - 1.6} ${y + 0.6}q1.6-2.2 3.2 0" stroke="${INK}" stroke-width="1.2" fill="none" stroke-linecap="round"/>`)
                .join("");
        case "error":
            return ex
                .map(
                    (x) =>
                        `<path d="M${x - 1.1} ${y - 1.1}l2.2 2.2M${x + 1.1} ${y - 1.1}l-2.2 2.2" stroke="${INK}" stroke-width="1" stroke-linecap="round"/>`,
                )
                .join("");
    }
}

function chibiMouth(state: BuddyState): string {
    const blush =
        `<ellipse cx="7.2" cy="16.4" rx="1.4" ry=".75" fill="#ff7aa5" opacity=".55"/><ellipse cx="16.8" cy="16.4" rx="1.4" ry=".75" fill="#ff7aa5" opacity=".55"/>` +
        `<path d="M6.6 16.7l.5-.7M7.5 16.7l.5-.7M16.2 16.7l.5-.7M17.1 16.7l.5-.7" stroke="#e8507f" stroke-width=".4" stroke-linecap="round" opacity=".8"/>`;
    const line = `stroke="${INK}" stroke-width=".9" stroke-linecap="round" stroke-linejoin="round" fill="none"`;
    switch (state) {
        case "idle":
            return `${blush}<path d="M11.3 17.6q.35.4.7 0q.35.4.7 0" ${line}/>`;
        case "working":
            return `${blush}<path d="M11.4 17.8h1.2" ${line}/>`;
        case "waiting":
            return `${blush}<path d="M11.3 17.4q.7 1.2 1.4 0Z" fill="#c2405f" ${line}/>`;
        case "done":
            return `${blush}<path d="M10.9 17.1q1.1 1.6 2.2 0Z" fill="#c2405f" ${line}/>`;
        case "error":
            return `<path d="M11.2 18q.4-.5.8 0t.8 0" ${line} stroke-width=".8"/>${sweat(19.4, 8.4)}`;
    }
}

const chibi = (id: string, name: string, back: (c: string) => string, front: (c: string) => string): BuddyShape => ({
    id,
    name,
    draw: (c, s) => `${back(c)}${FACE}${chibiEyes(s, darken(c, 0.45))}${chibiMouth(s)}${front(c)}`,
});

export const chibiPack: BuddyPack = {
    id: "chibi",
    name: "Chibi",
    blurb: "anime heads, one hair colour each",
    colors: HAIR,
    shapes: [
        chibi(
            "bob",
            "Bob",
            (c) =>
                `<path d="M3.4 13C3.4 6.6 7.2 3.4 12 3.4s8.6 3.2 8.6 9.6v6.4c0 1-.6 1.4-1.6 1.4H5c-1 0-1.6-.4-1.6-1.4Z" fill="${darken(c, 0.12)}" ${S()}/>`,
            (c) =>
                `<path d="M4.4 12.2C4.6 7.2 7.8 4.4 12 4.4s7.4 2.8 7.6 7.8c-1.4-.5-2.4-1.6-3-3-.6 1.5-1.8 2.6-3.6 3 .5-.9.5-2 0-2.8-1 1.7-2.9 2.7-5 2.9.7-.9.9-1.9.6-2.9-.8 1.4-2 2.4-3.2 3.3Z" fill="${c}" ${S()}/>`,
        ),
        chibi(
            "spiky",
            "Spiky",
            () => "",
            (c) =>
                `<path d="M3.6 13.2 4.4 8.6 2.4 7.4l2.8-.9-.4-3 3 1.4L9.6 2l2.4 2.4L14.4 2l1.8 2.9 3-1.4-.4 3 2.8.9-2 1.2.8 4.6-1.8-1.8-1.2.8-1-2.4-1.8 1.8-1.6-2-1.8 2-1.6-2-1.8 1.8-1-2.4-1.2.8-1.8 1.8Z" fill="${c}" ${S()}/>`,
        ),
        chibi(
            "hime",
            "Hime",
            (c) => `<path d="M3.2 12C3.2 6.4 7 3.4 12 3.4s8.8 3 8.8 8.6V22.4H3.2Z" fill="${darken(c, 0.12)}" ${S()}/>`,
            (c) =>
                `<path d="M4.4 11.2C4.6 6.8 7.8 4.4 12 4.4s7.4 2.4 7.6 6.8Z" fill="${c}" ${S()}/><path d="M4.4 11v8.4h2V11ZM17.6 11v8.4h2V11Z" fill="${c}" ${S()}/>`,
        ),
        chibi(
            "buns",
            "Buns",
            (c) =>
                `<circle cx="4.6" cy="5.6" r="2.8" fill="${darken(c, 0.08)}" ${S()}/><circle cx="19.4" cy="5.6" r="2.8" fill="${darken(c, 0.08)}" ${S()}/><path d="M4 13C4 7 7.6 4 12 4s8 3 8 9v3H4Z" fill="${darken(c, 0.12)}" ${S()}/>`,
            (c) =>
                `<path d="M4.6 12C4.8 7.4 7.8 4.6 12 4.6s7.2 2.8 7.4 7.4c-1.2-.3-2.2-1.2-2.8-2.4-.4 1.2-1.4 2.1-2.8 2.4.3-.8.3-1.6 0-2.4-.9 1.3-2.4 2.2-4.2 2.4.6-.7.8-1.6.6-2.4-.8 1.2-1.8 2-2.8 2.4Z" fill="${c}" ${S()}/>`,
        ),
        chibi(
            "hood",
            "Neko hood",
            (c) =>
                `<path d="M2.8 13.4c0-4 .8-7.2 2-10.2l3.6 2.6c1.1-.5 2.3-.7 3.6-.7s2.5.2 3.6.7l3.6-2.6c1.2 3 2 6.2 2 10.2 0 5.4-4 9-9.2 9s-9.2-3.6-9.2-9Z" fill="${c}" ${S()}/><path d="M5.2 5.4 6.6 6.4 5.6 8.2ZM18.8 5.4l-1.4 1 1 1.8Z" fill="#ffb3cc"/>`,
            () =>
                `<path d="M5.6 11.4c.6-3.4 3.2-5.2 6.4-5.2s5.8 1.8 6.4 5.2c-1-.3-1.8-1-2.3-2-.5 1-1.5 1.8-2.7 2 .3-.6.3-1.3 0-1.9-.8 1.1-2.1 1.8-3.6 1.9.5-.5.6-1.2.4-1.9-.7 1-1.6 1.6-2.6 1.9Z" fill="#5a4a66" ${S()}/>`,
        ),
        chibi(
            "side",
            "Side tail",
            (c) =>
                `<path d="M18.6 10.2c2.6.4 4 2.8 3.6 5.8-.3 2.4-1.6 4.6-3.4 5.8.6-2 .4-4-.6-5.6Z" fill="${darken(c, 0.08)}" ${S()}/><path d="M4 13C4 7 7.6 4 12 4s8 3 8 9v4H4Z" fill="${darken(c, 0.12)}" ${S()}/>`,
            (c) =>
                `<path d="M4.4 12.6C4.4 7.4 7.6 4.4 12 4.4c3.6 0 6.6 2 7.4 5.6-2.6-.2-5.2-1.4-6.8-3.4-.6 2-2.4 3.6-4.6 4.2.5-.8.7-1.6.6-2.4-1 1.6-2.6 3-4.2 4.2Z" fill="${c}" ${S()}/><circle cx="18.8" cy="10.4" r=".9" fill="#ff6f9a" ${S(0.7)}/>`,
        ),
    ],
};
