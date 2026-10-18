import { darken, INK, lighten, type BuddyPack, type BuddyShape, type BuddyState } from "../shared";

const VIBRANT = ["#ff3d8b", "#2f9bff", "#ffc21a", "#22d66b", "#8f4dff", "#ff6a1f", "#00d1c1", "#ff3b4e"] as const;
const PINK = "#ff7aa8";
const YELLOW = "#ffd36e";
const GREEN = "#7cf0a8";

interface Screen {
    x: number;
    y: number;
    w: number;
    h: number;
    r: number;
}

function screenFace(sc: Screen, state: BuddyState, glow: string): string {
    const { x, y, w, h } = sc;
    const ex = [x + w * 0.31, x + w * 0.69];
    const ey = y + h * 0.45;
    const my = y + h * 0.72;
    const cx = x + w / 2;
    const ew = Math.max(1.7, w * 0.15);
    const eh = ew * 1.3;
    const eye = (e: number, k = 1) =>
        `<rect x="${e - (ew * k) / 2}" y="${ey - (eh * k) / 2}" width="${ew * k}" height="${eh * k}" rx="${ew * 0.42 * k}" fill="${glow}"/><rect x="${e - (ew * k) / 2 + ew * 0.18}" y="${ey - (eh * k) / 2 + eh * 0.14}" width="${ew * 0.36}" height="${ew * 0.36}" rx=".2" fill="#fff"/>`;
    const cheek = (left: number, top: number, width: number) =>
        `<rect x="${left}" y="${top}" width="${width}" height="${width * 0.47}" rx="${width * 0.23}" fill="${PINK}" opacity=".85"/>`;
    const blush = cheek(ex[0] - ew * 1.4, ey + eh * 0.55, ew * 0.9) + cheek(ex[1] + ew * 0.5, ey + eh * 0.55, ew * 0.9);
    const line = `stroke="${glow}" stroke-width="${Math.max(0.8, w * 0.07)}" stroke-linecap="round" stroke-linejoin="round" fill="none"`;
    switch (state) {
        case "idle":
            return `${blush}<g class="bd-blink">${eye(ex[0])}${eye(ex[1])}</g><path d="M${cx - 1} ${my - 0.3}q.5.7 1 0q.5.7 1 0" ${line}/>`;
        case "working":
            return `${blush}<g class="bd-look">${eye(ex[0])}${eye(ex[1])}</g><ellipse cx="${cx}" cy="${my}" rx=".55" ry=".5" fill="${glow}"/>`;
        case "waiting":
            return `${blush}${eye(ex[0], 1.25)}${eye(ex[1], 1.25)}<ellipse cx="${cx}" cy="${my + 0.1}" rx=".7" ry=".8" fill="${glow}"/>`;
        case "done":
            return `${cheek(ex[0] - ew * 1.5, ey + eh * 0.4, ew)}${cheek(ex[1] + ew * 0.5, ey + eh * 0.4, ew)}${ex.map((e) => `<path d="M${e - ew * 0.75} ${ey + 0.5}q${ew * 0.75} -${eh * 0.9} ${ew * 1.5} 0" ${line}/>`).join("")}<path d="M${cx - 1.1} ${my - 0.5}q1.1 1.5 2.2 0Z" fill="${glow}" ${line}/>`;
        case "error":
            return `${ex.map((e) => `<path d="M${e - ew * 0.6} ${ey - ew * 0.6}l${ew * 1.2} ${ew * 1.2}M${e + ew * 0.6} ${ey - ew * 0.6}l-${ew * 1.2} ${ew * 1.2}" ${line}/>`).join("")}<path d="M${cx - 1.2} ${my}q.6-.6 1.2 0t1.2 0" ${line} stroke-width=".7"/>`;
    }
}

function screen(uid: string, sc: Screen, state: BuddyState, glow: string): string {
    const { x, y, w, h, r } = sc;
    return (
        `<clipPath id="${uid}c"><rect x="${x}" y="${y}" width="${w}" height="${h}" rx="${r}"/></clipPath>` +
        `<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="${r}" fill="url(#${uid}g)" stroke="${INK}" stroke-width=".9"/>` +
        `<g clip-path="url(#${uid}c)">` +
        (state === "working" ? `<rect class="bd-roll" x="${x}" y="${y}" width="${w}" height=".9" fill="${glow}" opacity=".18"/>` : "") +
        screenFace(sc, state, glow) +
        `<path d="M${x} ${y + h * 0.36}C${x + w * 0.3} ${y + h * 0.22} ${x + w * 0.6} ${y + h * 0.12} ${x + w} ${y + h * 0.1}V${y}H${x}Z" fill="#fff" opacity=".08"/>` +
        `<rect x="${x + w * 0.1}" y="${y + h * 0.12}" width="${w * 0.12}" height="${h * 0.08}" rx=".3" fill="#fff" opacity=".35"/>` +
        `</g>`
    );
}

const gradients = (uid: string, c: string) =>
    `<defs><linearGradient id="${uid}b" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${lighten(c, 0.28)}"/><stop offset=".5" stop-color="${c}"/><stop offset="1" stop-color="${darken(c, 0.22)}"/></linearGradient><radialGradient id="${uid}g" cx=".5" cy=".45" r=".75"><stop offset="0" stop-color="#2a2440"/><stop offset="1" stop-color="#0d0b16"/></radialGradient></defs>`;

type Parts = { uid: string; c: string; state: BuddyState; glow: string; shell: string; face: (sc: Screen) => string };
const rim = (d: string) => `<path d="${d}" fill="none" stroke="#fff" stroke-width=".7" opacity=".55" stroke-linecap="round"/>`;
const dot = (x: number, y: number, r: number, fill: string, width = 0.6) =>
    `<circle cx="${x}" cy="${y}" r="${r}" fill="${fill}" stroke="${INK}" stroke-width="${width}"/>`;

function gadget(id: string, name: string, draw: (p: Parts) => string): BuddyShape {
    return {
        id,
        name,
        draw: (c, state, uid) => {
            const glow = lighten(c, 0.45);
            const parts: Parts = {
                uid,
                c,
                state,
                glow,
                shell: `fill="url(#${uid}b)" stroke="${INK}" stroke-width="1.1"`,
                face: (sc) => screen(uid, sc, state, glow),
            };
            return gradients(uid, c) + draw(parts);
        },
    };
}

export const screens: BuddyPack = {
    id: "screens",
    name: "Screen pals",
    blurb: "a face on a little screen",
    colors: VIBRANT,
    shapes: [
        gadget(
            "telly",
            "Telly",
            ({ c, shell, face }) =>
                `<rect x="4.2" y="20" width="3" height="2.4" rx="1" fill="${darken(c, 0.35)}" stroke="${INK}" stroke-width=".9"/><rect x="16.8" y="20" width="3" height="2.4" rx="1" fill="${darken(c, 0.35)}" stroke="${INK}" stroke-width=".9"/><rect x="1.4" y="6.6" width="21.2" height="15" rx="4.6" ${shell}/>${rim("M4.6 7.6h14.8")}${face({ x: 3.2, y: 8.6, w: 13.6, h: 11, r: 3.2 })}${dot(19.6, 11.4, 1.25, darken(c, 0.3), 0.8)}<path d="M19.6 10.6v.8" stroke="${INK}" stroke-width=".6" stroke-linecap="round"/>${dot(19.6, 14.8, 1.25, darken(c, 0.3), 0.8)}<path d="M18.6 17.6h2M18.6 19h2" stroke="${darken(c, 0.45)}" stroke-width=".7" stroke-linecap="round"/>`,
        ),
        gadget(
            "pet",
            "Pet",
            ({ c, shell, face }) =>
                `<path d="M12 4C17 4 20.8 9.4 20.8 14.4c0 4.6-3.8 7.8-8.8 7.8s-8.8-3.2-8.8-7.8C3.2 9.4 7 4 12 4Z" ${shell}/>${rim("M8 7.2C9.2 5.9 10.5 5.2 12 5.2")}<circle cx="6.2" cy="11.2" r=".8" fill="#fff" opacity=".6"/><circle cx="17.8" cy="9.8" r="1" fill="#fff" opacity=".6"/>${face({ x: 6.6, y: 8.2, w: 10.8, h: 8.4, r: 2.4 })}${dot(9.4, 19, 1, darken(c, 0.3), 0.7)}${dot(12, 19.7, 1, darken(c, 0.3), 0.7)}${dot(14.6, 19, 1, darken(c, 0.3), 0.7)}`,
        ),
        gadget(
            "pocket",
            "Pocket",
            ({ c, shell, face }) =>
                `<path d="M6.4 1.4h11.2a2.4 2.4 0 0 1 2.4 2.4v14a4.6 4.6 0 0 1-4.6 4.6H6.4A2.4 2.4 0 0 1 4 20V3.8a2.4 2.4 0 0 1 2.4-2.4Z" ${shell}/>${rim("M6.6 2.5h10.8")}${face({ x: 5.8, y: 3.2, w: 12.4, h: 9.8, r: 2 })}<path d="M8.8 14.9v3.4M7.1 16.6h3.4" stroke="${darken(c, 0.55)}" stroke-width="1.5" stroke-linecap="round"/>${dot(16.4, 15.6, 1.15, PINK, 0.7)}${dot(14.2, 17.6, 1.15, YELLOW, 0.7)}`,
        ),
        gadget(
            "desktop",
            "Desktop",
            ({ c, shell, face }) =>
                `<path d="M8 22.4h8l-1-3.6H9Z" fill="${darken(c, 0.25)}" stroke="${INK}" stroke-width=".9" stroke-linejoin="round"/><rect x="2.4" y="2.2" width="19.2" height="17.4" rx="3.6" ${shell}/>${rim("M5.2 3.2h13.6")}${face({ x: 4.4, y: 4.2, w: 15.2, h: 10.8, r: 2.4 })}<rect x="5" y="16.6" width="4.4" height=".9" rx=".45" fill="${darken(c, 0.4)}"/>${dot(18.2, 17, 0.75, GREEN, 0.5)}`,
        ),
        gadget(
            "laptop",
            "Laptop",
            ({ c, shell, face }) =>
                `<rect x="3.4" y="3.4" width="17.2" height="13" rx="2.2" ${shell}/>${face({ x: 5, y: 5, w: 14, h: 9.8, r: 1.4 })}<path d="M1 17.4h22l-1.1 2.5c-.3.6-.8.9-1.4.9H3.5c-.6 0-1.1-.3-1.4-.9Z" ${shell}/><rect x="10" y="17.4" width="4" height=".9" rx=".4" fill="${darken(c, 0.4)}"/>`,
        ),
        gadget(
            "phone",
            "Phone",
            ({ shell, face }) =>
                `<rect x="5.8" y="1.2" width="12.4" height="21.6" rx="3.2" ${shell}/>${face({ x: 7.2, y: 2.8, w: 9.6, h: 18.4, r: 2.2 })}<rect x="10.6" y="3.6" width="2.8" height="1" rx=".5" fill="#000"/>`,
        ),
        gadget(
            "watch",
            "Watch",
            ({ c, shell, face }) =>
                `<rect x="7.8" y=".6" width="8.4" height="6" rx="1.6" fill="${darken(c, 0.3)}" stroke="${INK}" stroke-width=".9"/><rect x="7.8" y="17.4" width="8.4" height="6" rx="1.6" fill="${darken(c, 0.3)}" stroke="${INK}" stroke-width=".9"/><rect x="19.2" y="9.4" width="1.6" height="2.8" rx=".7" fill="${darken(c, 0.2)}" stroke="${INK}" stroke-width=".7"/><rect x="4.4" y="4.4" width="15.2" height="15.2" rx="4.6" ${shell}/>${face({ x: 6, y: 6, w: 12, h: 12, r: 3.2 })}`,
        ),
        gadget(
            "arcade",
            "Arcade",
            ({ glow, shell, face }) =>
                `<path d="M5 1.4h14v4.6l-1.2 1.4v7.2l2.4 2.6v6.2H3.8v-6.2l2.4-2.6V7.4L5 6Z" ${shell}/><rect x="6.2" y="2.5" width="11.6" height="2.6" rx=".8" fill="${glow}" stroke="${INK}" stroke-width=".7"/>${face({ x: 7.2, y: 7.6, w: 9.6, h: 7.4, r: 1 })}<path d="M9.2 19.4v-2" stroke="${INK}" stroke-width=".9" stroke-linecap="round"/>${dot(9.2, 17.2, 0.95, PINK)}${dot(13.6, 18.8, 0.75, YELLOW)}${dot(15.6, 18.8, 0.75, GREEN)}`,
        ),
        gadget(
            "handheld",
            "Handheld",
            ({ c, shell, face }) =>
                `<rect x=".8" y="5.2" width="22.4" height="13.6" rx="5.2" ${shell}/>${rim("M5 6.2h14")}${face({ x: 6.2, y: 7, w: 11.6, h: 10, r: 1.4 })}<path d="M3.6 10.4v3.2M2 12h3.2" stroke="${darken(c, 0.55)}" stroke-width="1.3" stroke-linecap="round"/>${dot(20.4, 10.8, 0.95, PINK)}${dot(19, 13.2, 0.95, YELLOW)}`,
        ),
        gadget(
            "classic",
            "Classic",
            ({ c, shell, face }) =>
                `<rect x="4" y="1.2" width="16" height="21.6" rx="2.6" ${shell}/>${rim("M6.4 2.2h11.2")}${face({ x: 5.8, y: 3, w: 12.4, h: 10.2, r: 1.6 })}<rect x="12.4" y="16.4" width="4.8" height="1" rx=".5" fill="${darken(c, 0.5)}"/>${dot(7.4, 17, 0.75, GREEN, 0.5)}<path d="M4.6 20.2h14.8" stroke="${darken(c, 0.3)}" stroke-width=".7"/>`,
        ),
        gadget(
            "camera",
            "Camera",
            ({ c, shell, face }) =>
                `<rect x="6.4" y="3.4" width="6.8" height="3.6" rx="1.2" ${shell}/><rect x="1.4" y="5.6" width="21.2" height="15" rx="3.6" ${shell}/>${face({ x: 3.2, y: 7.6, w: 12.8, h: 11, r: 2 })}${dot(19.4, 10, 1.5, darken(c, 0.3), 0.7)}<circle cx="19.4" cy="10" r=".5" fill="${INK}"/>${dot(19.4, 14.6, 0.8, PINK, 0.5)}<path d="M18.4 17.4h2M18.4 18.6h2" stroke="${darken(c, 0.45)}" stroke-width=".6" stroke-linecap="round"/>`,
        ),
        gadget(
            "washer",
            "Washer",
            ({ c, shell, face }) =>
                `<rect x="2.6" y="1.2" width="18.8" height="21.4" rx="3.4" ${shell}/><path d="M2.6 6h18.8" stroke="${INK}" stroke-width=".8"/><rect x="4.4" y="2.8" width="5.4" height="1.6" rx=".5" fill="${darken(c, 0.35)}"/>${dot(17.6, 3.6, 1.1, darken(c, 0.3))}<circle cx="12" cy="14.2" r="7" fill="${darken(c, 0.25)}" stroke="${INK}" stroke-width=".9"/>${face({ x: 6.4, y: 8.6, w: 11.2, h: 11.2, r: 5.6 })}`,
        ),
        gadget(
            "microwave",
            "Microwave",
            ({ c, glow, shell, face }) =>
                `<rect x="3" y="19.6" width="2.6" height="1.8" rx=".6" fill="${darken(c, 0.35)}"/><rect x="18.4" y="19.6" width="2.6" height="1.8" rx=".6" fill="${darken(c, 0.35)}"/><rect x=".8" y="3.8" width="22.4" height="16.2" rx="3" ${shell}/>${rim("M3.4 4.8h17.2")}${face({ x: 2.6, y: 5.8, w: 13.8, h: 12.2, r: 2 })}<rect x="17.8" y="6.2" width="3.8" height="2" rx=".5" fill="#0d0b16" stroke="${INK}" stroke-width=".5"/><path d="M18.5 7.2h2.4" stroke="${glow}" stroke-width=".7" stroke-dasharray=".6 .3"/>${[10.4, 12.6, 14.8].map((y) => `<circle cx="18.8" cy="${y}" r=".5" fill="${darken(c, 0.45)}"/><circle cx="20.6" cy="${y}" r=".5" fill="${darken(c, 0.45)}"/>`).join("")}`,
        ),
        gadget(
            "boombox",
            "Boombox",
            ({ c, shell, face }) =>
                `<rect x="1" y="6" width="22" height="13.6" rx="3.6" ${shell}/>${rim("M4.4 7h15.2")}<circle cx="5.2" cy="13.6" r="3.1" fill="#0d0b16" stroke="${INK}" stroke-width=".8"/><circle cx="5.2" cy="13.6" r="1.1" fill="${darken(c, 0.2)}"/><circle cx="18.8" cy="13.6" r="3.1" fill="#0d0b16" stroke="${INK}" stroke-width=".8"/><circle cx="18.8" cy="13.6" r="1.1" fill="${darken(c, 0.2)}"/>${face({ x: 8.8, y: 8.2, w: 6.4, h: 9.4, r: 1.4 })}`,
        ),
    ],
};
