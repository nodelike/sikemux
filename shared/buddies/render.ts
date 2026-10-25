import { twinkle, type BuddyPack, type BuddyShape, type BuddyState } from "./shared";

const MOTION: Record<BuddyState, string> = { idle: "", working: "bd-bob", waiting: "bd-hop", done: "", error: "bd-shake" };

/** The whole-body motion a state plays; the Mac runs it on the drawing as a whole, where the compositor can take it. */
export const bodyMotion = (state: BuddyState): string => MOTION[state];
export const FULL_BOX = "0 0 24 24";
const PAD = 0.7;

/** The square around a drawing's bounds, with a little room, that crops it to fill its slot. */
export function boxAround(bounds: { x: number; y: number; width: number; height: number }): string {
    const side = Math.max(bounds.width, bounds.height) + PAD * 2;
    const f = (n: number) => n.toFixed(2);
    return `${f(bounds.x + bounds.width / 2 - side / 2)} ${f(bounds.y + bounds.height / 2 - side / 2)} ${f(side)} ${f(side)}`;
}

/** What a shape is measured on to crop it. */
export const measuredDrawing = (pack: BuddyPack, shape: BuddyShape): string => `<g>${shape.draw(pack.colors[0], "idle", "measure")}</g>`;

const boxes = new Map<string, string>();

/* Shapes fill different parts of their 24×24 box, so each is cropped to its own drawing and every buddy fills the slot it is given. */
export function buddyBox(pack: BuddyPack, shape: BuddyShape): string {
    const key = `${pack.id}/${shape.id}`;
    const known = boxes.get(key);
    if (known) return known;
    let box = FULL_BOX;
    if (typeof document !== "undefined") {
        const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
        svg.setAttribute("style", "position:fixed;left:-9999px;top:0;width:24px;height:24px;visibility:hidden");
        svg.innerHTML = measuredDrawing(pack, shape);
        document.body.appendChild(svg);
        try {
            const g = svg.firstElementChild as SVGGraphicsElement | null;
            const b = typeof g?.getBBox === "function" ? g.getBBox() : null;
            if (b && b.width > 0 && b.height > 0) box = boxAround(b);
        } finally {
            svg.remove();
        }
    }
    boxes.set(key, box);
    return box;
}

/** `box` is measured on this page unless given; a phone cannot measure, so it passes the one the app published. */
export function buddyMarkup(pack: BuddyPack, shape: BuddyShape, color: string, state: BuddyState, uid: string, box = buddyBox(pack, shape)): string {
    const [x, y, side] = box.split(" ").map(Number);
    const sparkle = state === "done" ? twinkle(x + side - side * 0.08, y + side * 0.1) : "";
    return `<svg viewBox="${box}" aria-hidden="true"><g class="${MOTION[state]}">${shape.draw(color, state, uid)}${sparkle}</g></svg>`;
}
