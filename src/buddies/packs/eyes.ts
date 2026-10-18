import { darken, eyeball, INK, lighten, NEON, PASTEL, type BuddyPack, type BuddyShape, type BuddyState } from "../shared";

const FACE = "#14121b";
const RIM = `fill="${FACE}" stroke="#3a3546" stroke-width="1.2"`;

function glowEyes(color: string, state: BuddyState, cy: number): string {
    const xs = [8.1, 15.9];
    const rect = (x: number, w: number, h: number, rx: number) =>
        `<rect x="${x - w / 2}" y="${cy - h / 2}" width="${w}" height="${h}" rx="${rx}" fill="${color}"/>`;
    let eyes = "";
    if (state === "idle") eyes = `<g class="bd-glance"><g class="bd-blink">${xs.map((x) => rect(x, 4.6, 6.6, 1.9)).join("")}</g></g>`;
    if (state === "working") eyes = `<g class="bd-look">${xs.map((x) => rect(x, 4.8, 2.8, 1.2)).join("")}</g>`;
    if (state === "waiting")
        eyes = xs.map((x) => rect(x, 5.4, 7.8, 2.7) + `<circle cx="${x - 1}" cy="${cy - 2}" r=".9" fill="#fff" opacity=".9"/>`).join("");
    if (state === "done")
        eyes = xs
            .map(
                (x) =>
                    `<path d="M${x - 2.3} ${cy + 1.4}Q${x} ${cy - 3.8} ${x + 2.3} ${cy + 1.4}" stroke="${color}" stroke-width="2.3" fill="none" stroke-linecap="round"/>`,
            )
            .join("");
    if (state === "error")
        eyes =
            xs.map((x) => rect(x, 4.6, 5.4, 1.6)).join("") +
            `<path d="M${xs[0] - 3} ${cy - 3.4}H${xs[0] + 3}V${cy - 0.2}ZM${xs[1] + 3} ${cy - 3.4}H${xs[1] - 3}V${cy - 0.2}Z" fill="${FACE}"/>`;
    return `<rect x="4.4" y="${cy - 4.6}" width="15.2" height="9.2" rx="4" fill="${color}" opacity=".1"/>${eyes}`;
}

const glowShape = (id: string, name: string, face: string, cy: number): BuddyShape => ({
    id,
    name,
    draw: (color, state) => `${face}${glowEyes(color, state, cy)}`,
});

export const glow: BuddyPack = {
    id: "glow",
    name: "Glow",
    blurb: "big glowing eyes on a dark face",
    colors: NEON,
    shapes: [
        glowShape("squircle", "Squircle", `<rect x="2" y="3.5" width="20" height="17" rx="6.5" ${RIM}/>`, 12),
        glowShape("orb", "Orb", `<circle cx="12" cy="12" r="10" ${RIM}/>`, 12),
        glowShape("pill", "Pill", `<rect x=".8" y="5.6" width="22.4" height="12.8" rx="6.4" ${RIM}/>`, 12),
        glowShape("monitor", "Monitor", `<rect x="1.4" y="4" width="21.2" height="16" rx="3.4" ${RIM}/>`, 12),
        glowShape("dome", "Dome", `<path d="M2 15C2 8 6.4 3.6 12 3.6S22 8 22 15v2.4c0 1.6-1.2 2.6-2.8 2.6H4.8C3.2 20 2 19 2 17.4Z" ${RIM}/>`, 13.2),
    ],
};

const lashes = (xs: number[]) =>
    xs
        .map((x) => `<path d="M${x - 3.4} 8.6l-1-1.2M${x} 7.6v-1.6M${x + 3.4} 8.6l1-1.2" stroke="${INK}" stroke-width="1" stroke-linecap="round"/>`)
        .join("");

export const googly: BuddyPack = {
    id: "googly",
    name: "Googly",
    blurb: "nothing but eyes",
    colors: NEON,
    shapes: [
        { id: "pair", name: "Pair", draw: (c, s) => eyeball(7.4, 12, 4.8, c, s) + eyeball(16.6, 12, 4.8, c, s) },
        { id: "lashes", name: "Lashes", draw: (c, s) => eyeball(7.4, 13, 4.6, c, s) + eyeball(16.6, 13, 4.6, c, s) + lashes([7.4, 16.6]) },
        {
            id: "sleepy",
            name: "Sleepy",
            draw: (c, s) =>
                [7.4, 16.6]
                    .map(
                        (x) =>
                            eyeball(x, 12, 4.8, c, s) +
                            (s === "done"
                                ? ""
                                : `<path d="M${x - 4.8} 11.2A4.8 4.8 0 0 1 ${x + 4.8} 11.2Z" fill="${lighten(c, 0.25)}" stroke="${INK}" stroke-width="1"/>`),
                    )
                    .join(""),
        },
        {
            id: "specs",
            name: "Specs",
            draw: (c, s) =>
                eyeball(7.2, 12, 4, c, s) +
                eyeball(16.8, 12, 4, c, s) +
                `<circle cx="7.2" cy="12" r="5.3" fill="none" stroke="${c}" stroke-width="1.3"/><circle cx="16.8" cy="12" r="5.3" fill="none" stroke="${c}" stroke-width="1.3"/><path d="M10.6 11.2q1.4-1 2.8 0" stroke="${c}" stroke-width="1.3" fill="none"/>`,
        },
        { id: "trio", name: "Trio", draw: (c, s) => eyeball(6.2, 14.4, 4, c, s) + eyeball(17.8, 14.4, 4, c, s) + eyeball(12, 7.4, 4.2, c, s) },
        { id: "big", name: "Big eye", draw: (c, s) => eyeball(12, 12, 9.4, c, s) },
    ],
};

function fuzz(cx: number, cy: number, r: number, spikes: number, jitter: number, seed: number): string {
    let state = seed;
    const rand = () => (state = (state * 9301 + 49297) % 233280) / 233280;
    let d = "";
    for (let i = 0; i < spikes * 2; i++) {
        const a = (i / (spikes * 2)) * Math.PI * 2;
        const rr = i % 2 ? r * (1 - jitter * 0.4) : r * (1 + jitter * (0.6 + rand() * 0.6));
        d += (i ? "L" : "M") + (cx + Math.cos(a) * rr).toFixed(2) + " " + (cy + Math.sin(a) * rr).toFixed(2);
    }
    return d + "Z";
}
const FUZZ = {
    soot: fuzz(12, 12.4, 8, 30, 0.18, 7),
    fluff: fuzz(12, 12.4, 8.6, 22, 0.12, 3),
    konpeito: fuzz(11, 12, 7.4, 28, 0.18, 11),
    candy: fuzz(19.4, 17.4, 2.4, 6, 0.35, 5),
    puff: fuzz(12, 12.6, 9, 14, 0.1, 9),
};
const legs = (c: string) => `<path d="M9 19.6l-.8 2.6M15 19.6l.8 2.6" stroke="${c}" stroke-width=".9" stroke-linecap="round"/>`;
const sootEyes = (s: BuddyState) => eyeball(9.3, 12, 2.9, null, s) + eyeball(14.7, 12, 2.9, null, s);
const blush = (xl: number, xr: number, y: number) =>
    `<ellipse cx="${xl}" cy="${y}" rx="1.2" ry=".7" fill="#ff5f8f" opacity=".5"/><ellipse cx="${xr}" cy="${y}" rx="1.2" ry=".7" fill="#ff5f8f" opacity=".5"/>`;

export const soot: BuddyPack = {
    id: "soot",
    name: "Soot",
    blurb: "fuzzballs that stare",
    colors: NEON,
    shapes: [
        {
            id: "soot",
            name: "Soot",
            draw: (c, s) => `${legs(darken(c, 0.4))}<path d="${FUZZ.soot}" fill="${darken(c, 0.72)}" stroke="${c}" stroke-width=".7"/>${sootEyes(s)}`,
        },
        {
            id: "fluff",
            name: "Fluff",
            draw: (c, s) =>
                `<path d="${FUZZ.fluff}" fill="${lighten(c, 0.25)}" stroke="${INK}" stroke-width=".9" stroke-linejoin="round"/>${sootEyes(s)}${blush(6.6, 17.4, 15.2)}`,
        },
        {
            id: "konpeito",
            name: "Konpeito",
            draw: (c, s) =>
                `${legs(darken(c, 0.4))}<path d="${FUZZ.konpeito}" fill="${darken(c, 0.72)}" stroke="${c}" stroke-width=".7"/>${eyeball(8.5, 11.6, 2.7, null, s)}${eyeball(13.5, 11.6, 2.7, null, s)}<path d="${FUZZ.candy}" fill="${c}" stroke="${INK}" stroke-width=".6" stroke-linejoin="round"/>`,
        },
        {
            id: "puff",
            name: "Puff",
            draw: (c, s) =>
                `<path d="${FUZZ.puff}" fill="${c}" stroke="${INK}" stroke-width=".9" stroke-linejoin="round"/>${eyeball(8.8, 12, 3.4, null, s)}${eyeball(15.2, 12, 3.4, null, s)}`,
        },
    ],
};

export const peekers: BuddyPack = {
    id: "peekers",
    name: "Peekers",
    blurb: "eyes peeking over the edge",
    colors: PASTEL,
    shapes: [
        {
            id: "peek",
            name: "Peek",
            draw: (c, s) =>
                `<path d="M1.6 22V14.4C1.6 8.2 6.2 4.6 12 4.6s10.4 3.6 10.4 9.8V22Z" fill="${c}" stroke="${INK}" stroke-width="1.1"/>${eyeball(8.4, 11.4, 3.5, null, s, { up: true })}${eyeball(15.6, 11.4, 3.5, null, s, { up: true })}`,
        },
        {
            id: "frog",
            name: "Frog",
            draw: (c, s) =>
                `<path d="M1 22v-5.4C1 12.6 4 10.4 7 9.8h10c3 .6 6 2.8 6 6.8V22Z" fill="${c}" stroke="${INK}" stroke-width="1.1"/><circle cx="7.2" cy="8.8" r="4.4" fill="${c}" stroke="${INK}" stroke-width="1.1"/><circle cx="16.8" cy="8.8" r="4.4" fill="${c}" stroke="${INK}" stroke-width="1.1"/><path d="M4 10.6h16v3H4Z" fill="${c}"/>${eyeball(7.2, 8.6, 3, null, s)}${eyeball(16.8, 8.6, 3, null, s)}<path d="M9.4 16.4q2.6 1.6 5.2 0" stroke="${INK}" stroke-width=".9" fill="none" stroke-linecap="round"/>${blush(4.8, 19.2, 15.4)}`,
        },
        {
            id: "owl",
            name: "Owl",
            draw: (c, s) =>
                `<path d="M12 3.4c5.2 0 8.8 3.8 8.8 9.4 0 5.4-3.8 9.2-8.8 9.2s-8.8-3.8-8.8-9.2C3.2 7.2 6.8 3.4 12 3.4Z" fill="${c}" stroke="${INK}" stroke-width="1.1"/><circle cx="8.2" cy="11" r="4.2" fill="${lighten(c, 0.6)}"/><circle cx="15.8" cy="11" r="4.2" fill="${lighten(c, 0.6)}"/>${eyeball(8.2, 11, 3.1, null, s)}${eyeball(15.8, 11, 3.1, null, s)}<path d="M11 14.6h2l-1 1.6Z" fill="#ffb43d" stroke="${INK}" stroke-width=".6" stroke-linejoin="round"/><path d="M8.6 18.6l.8.8.8-.8M13.8 18.6l.8.8.8-.8" stroke="${darken(c, 0.35)}" stroke-width=".7" fill="none"/>`,
        },
    ],
};

function cyclopsShape(id: string, name: string, body: string, x: number, y: number, r: number, my: number): BuddyShape {
    return {
        id,
        name,
        draw: (c, s) => {
            const mouth =
                s === "done"
                    ? `<path d="M${x - 1.4} ${my - 0.5}q1.4 1.8 2.8 0Z" fill="${INK}"/>`
                    : s === "error"
                      ? `<path d="M${x - 1.4} ${my}q.7-.7 1.4 0t1.4 0" stroke="${INK}" stroke-width=".9" fill="none"/>`
                      : `<path d="M${x - 0.9} ${my - 0.3}q.9.9 1.8 0" stroke="${INK}" stroke-width=".9" fill="none" stroke-linecap="round"/>`;
            return `${body.replace("/>", ` fill="${c}" stroke="${INK}" stroke-width="1.1" stroke-linejoin="round"/>`)}${eyeball(x, y, r, darken(c, 0.25), s)}${mouth}${blush(x - r - 1, x + r + 1, my - 1)}`;
        },
    };
}

export const cyclops: BuddyPack = {
    id: "cyclops",
    name: "Cyclops",
    blurb: "one big eye",
    colors: PASTEL,
    shapes: [
        cyclopsShape("orb", "Orb", `<circle cx="12" cy="12.4" r="9.6"/>`, 12, 11.2, 5.2, 17.4),
        cyclopsShape("drop", "Drop", `<path d="M12 2.6c3 3.6 9 7.4 9 12.6 0 3.8-3 6.6-9 6.6s-9-2.8-9-6.6c0-5.2 6-9 9-12.6Z"/>`, 12, 14, 4.6, 19),
        cyclopsShape(
            "squish",
            "Squish",
            `<path d="M1.6 17.4C1.6 10.4 6.2 6.4 12 6.4s10.4 4 10.4 11c0 3-2.2 4.6-5 4.6H6.6c-2.8 0-5-1.6-5-4.6Z"/>`,
            12,
            13.6,
            4.6,
            19.2,
        ),
        cyclopsShape(
            "spook",
            "Spook",
            `<path d="M3.6 12.6c0-4.8 3.8-8 8.4-8s8.4 3.2 8.4 8v9l-2.1-1.5-2.1 1.5-2.1-1.5-2.1 1.5-2.1-1.5-2.1 1.5-2.1-1.5-2.1 1.5Z"/>`,
            12,
            11.6,
            4.8,
            17,
        ),
    ],
};

const DOTS: Record<BuddyState, readonly string[]> = {
    idle: ["111", "111", "111", "111"],
    working: ["000", "111", "111", "000"],
    waiting: ["111", "101", "101", "111"],
    done: ["000", "010", "101", "000"],
    error: ["101", "010", "101", "000"],
};
function dots(cx: number, cy: number, color: string, state: BuddyState): string {
    let out = "";
    for (let row = 0; row < 4; row++) {
        for (let col = 0; col < 3; col++) {
            const on = DOTS[state][row][col] === "1";
            out += `<circle cx="${cx + (col - 1) * 1.55}" cy="${cy + (row - 1.5) * 1.55}" r=".58" fill="${color}" opacity="${on ? 1 : 0.12}"/>`;
        }
    }
    return out;
}
const MATRIX_MOTION: Record<BuddyState, string> = { idle: "bd-blink", working: "bd-look", waiting: "bd-flash", done: "", error: "" };
const matrixShape = (id: string, name: string, face: string): BuddyShape => ({
    id,
    name,
    draw: (c, s) => `${face}<g class="${MATRIX_MOTION[s]}">${dots(7.9, 12, c, s)}${dots(16.1, 12, c, s)}</g>`,
});

export const matrix: BuddyPack = {
    id: "matrix",
    name: "Matrix",
    blurb: "LED dot eyes",
    colors: NEON,
    shapes: [
        matrixShape("matrix", "Matrix", `<rect x="1.6" y="3.6" width="20.8" height="16.8" rx="6" ${RIM}/>`),
        matrixShape("round", "Round", `<circle cx="12" cy="12" r="10.2" ${RIM}/>`),
        matrixShape("visor", "Visor", `<rect x=".6" y="6" width="22.8" height="12" rx="6" ${RIM}/>`),
    ],
};
