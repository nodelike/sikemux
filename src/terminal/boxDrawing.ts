/*
 * Box drawing, block and powerline characters are drawn as shapes filling
 * the cell, as xterm's WebGL renderer and Ghostty do, so borders and rules
 * join up whatever the font's own glyphs and the row height are.
 */

const LIGHT = 1;
const HEAVY = 2;
const DOUBLE = 3;

/** Line weights toward the top, right, bottom and left edges of the cell. */
type Arms = readonly [number, number, number, number];

const ARMS: Record<number, Arms> = {
    0x2500: [0, LIGHT, 0, LIGHT],
    0x2501: [0, HEAVY, 0, HEAVY],
    0x2502: [LIGHT, 0, LIGHT, 0],
    0x2503: [HEAVY, 0, HEAVY, 0],
    0x250c: [0, LIGHT, LIGHT, 0],
    0x250d: [0, HEAVY, LIGHT, 0],
    0x250e: [0, LIGHT, HEAVY, 0],
    0x250f: [0, HEAVY, HEAVY, 0],
    0x2510: [0, 0, LIGHT, LIGHT],
    0x2511: [0, 0, LIGHT, HEAVY],
    0x2512: [0, 0, HEAVY, LIGHT],
    0x2513: [0, 0, HEAVY, HEAVY],
    0x2514: [LIGHT, LIGHT, 0, 0],
    0x2515: [LIGHT, HEAVY, 0, 0],
    0x2516: [HEAVY, LIGHT, 0, 0],
    0x2517: [HEAVY, HEAVY, 0, 0],
    0x2518: [LIGHT, 0, 0, LIGHT],
    0x2519: [LIGHT, 0, 0, HEAVY],
    0x251a: [HEAVY, 0, 0, LIGHT],
    0x251b: [HEAVY, 0, 0, HEAVY],
    0x251c: [LIGHT, LIGHT, LIGHT, 0],
    0x251d: [LIGHT, HEAVY, LIGHT, 0],
    0x2520: [HEAVY, LIGHT, HEAVY, 0],
    0x2523: [HEAVY, HEAVY, HEAVY, 0],
    0x2524: [LIGHT, 0, LIGHT, LIGHT],
    0x2525: [LIGHT, 0, LIGHT, HEAVY],
    0x2528: [HEAVY, 0, HEAVY, LIGHT],
    0x252b: [HEAVY, 0, HEAVY, HEAVY],
    0x252c: [0, LIGHT, LIGHT, LIGHT],
    0x252f: [0, HEAVY, LIGHT, HEAVY],
    0x2530: [0, LIGHT, HEAVY, LIGHT],
    0x2533: [0, HEAVY, HEAVY, HEAVY],
    0x2534: [LIGHT, LIGHT, 0, LIGHT],
    0x2537: [LIGHT, HEAVY, 0, HEAVY],
    0x2538: [HEAVY, LIGHT, 0, LIGHT],
    0x253b: [HEAVY, HEAVY, 0, HEAVY],
    0x253c: [LIGHT, LIGHT, LIGHT, LIGHT],
    0x253f: [LIGHT, HEAVY, LIGHT, HEAVY],
    0x2542: [HEAVY, LIGHT, HEAVY, LIGHT],
    0x254b: [HEAVY, HEAVY, HEAVY, HEAVY],
    0x2550: [0, DOUBLE, 0, DOUBLE],
    0x2551: [DOUBLE, 0, DOUBLE, 0],
    0x2554: [0, DOUBLE, DOUBLE, 0],
    0x2557: [0, 0, DOUBLE, DOUBLE],
    0x255a: [DOUBLE, DOUBLE, 0, 0],
    0x255d: [DOUBLE, 0, 0, DOUBLE],
    0x2560: [DOUBLE, DOUBLE, DOUBLE, 0],
    0x2563: [DOUBLE, 0, DOUBLE, DOUBLE],
    0x2566: [0, DOUBLE, DOUBLE, DOUBLE],
    0x2569: [DOUBLE, DOUBLE, 0, DOUBLE],
    0x256c: [DOUBLE, DOUBLE, DOUBLE, DOUBLE],
    0x2574: [0, 0, 0, LIGHT],
    0x2575: [LIGHT, 0, 0, 0],
    0x2576: [0, LIGHT, 0, 0],
    0x2577: [0, 0, LIGHT, 0],
    0x2578: [0, 0, 0, HEAVY],
    0x2579: [HEAVY, 0, 0, 0],
    0x257a: [0, HEAVY, 0, 0],
    0x257b: [0, 0, HEAVY, 0],
};

/** Rounded corners, by the two edges they join. */
const ARCS: Record<number, readonly [number, number]> = {
    0x256d: [1, 2],
    0x256e: [2, 3],
    0x256f: [0, 3],
    0x2570: [0, 1],
};

export function isDrawnGlyph(code: number): boolean {
    return (code >= 0x2500 && code <= 0x259f) || (code >= 0xe0b0 && code <= 0xe0b3);
}

/** Draws `code` filling its cell, answering false for a character left to the font. */
export function drawGlyph(ctx: CanvasRenderingContext2D, code: number, x: number, y: number, width: number, height: number, stroke: number): boolean {
    const arms = ARMS[code];
    if (arms) {
        drawArms(ctx, arms, x, y, width, height, stroke);
        return true;
    }
    const arc = ARCS[code];
    if (arc) {
        drawArc(ctx, arc, x, y, width, height, stroke);
        return true;
    }
    return drawBlock(ctx, code, x, y, width, height);
}

function drawArms(ctx: CanvasRenderingContext2D, arms: Arms, x: number, y: number, width: number, height: number, stroke: number): void {
    const cx = x + Math.floor((width - stroke) / 2);
    const cy = y + Math.floor((height - stroke) / 2);
    const thick = (weight: number) => (weight === HEAVY ? stroke * 2 : stroke);
    const [top, right, bottom, left] = arms;
    const doubled = arms.some((weight) => weight === DOUBLE);
    if (doubled) {
        const gap = stroke;
        const horizontal = left === DOUBLE || right === DOUBLE;
        const vertical = top === DOUBLE || bottom === DOUBLE;
        for (const offset of [-gap, gap]) {
            if (horizontal) {
                const from = left ? x : cx;
                const to = right ? x + width : cx + stroke;
                ctx.fillRect(from, cy + offset, to - from, stroke);
            }
            if (vertical) {
                const from = top ? y : cy;
                const to = bottom ? y + height : cy + stroke;
                ctx.fillRect(cx + offset, from, stroke, to - from);
            }
        }
        return;
    }
    if (left) {
        const t = thick(left);
        ctx.fillRect(x, cy - (t - stroke) / 2, cx - x + stroke, t);
    }
    if (right) {
        const t = thick(right);
        ctx.fillRect(cx, cy - (t - stroke) / 2, x + width - cx, t);
    }
    if (top) {
        const t = thick(top);
        ctx.fillRect(cx - (t - stroke) / 2, y, t, cy - y + stroke);
    }
    if (bottom) {
        const t = thick(bottom);
        ctx.fillRect(cx - (t - stroke) / 2, cy, t, y + height - cy);
    }
}

function drawArc(
    ctx: CanvasRenderingContext2D,
    edges: readonly [number, number],
    x: number,
    y: number,
    width: number,
    height: number,
    stroke: number,
): void {
    const cx = x + Math.floor((width - stroke) / 2) + stroke / 2;
    const cy = y + Math.floor((height - stroke) / 2) + stroke / 2;
    const ends: Record<number, [number, number]> = { 0: [cx, y], 1: [x + width, cy], 2: [cx, y + height], 3: [x, cy] };
    const [a, b] = edges.map((edge) => ends[edge]);
    const radius = Math.min(width, height) / 2;
    ctx.save();
    ctx.lineWidth = stroke;
    ctx.strokeStyle = ctx.fillStyle;
    ctx.beginPath();
    ctx.moveTo(a[0], a[1]);
    ctx.arcTo(cx, cy, b[0], b[1], radius);
    ctx.lineTo(b[0], b[1]);
    ctx.stroke();
    ctx.restore();
}

function drawBlock(ctx: CanvasRenderingContext2D, code: number, x: number, y: number, width: number, height: number): boolean {
    if (code === 0x2588) {
        ctx.fillRect(x, y, width, height);
        return true;
    }
    if (code >= 0x2581 && code <= 0x2587) {
        const part = Math.round((height * (code - 0x2580)) / 8);
        ctx.fillRect(x, y + height - part, width, part);
        return true;
    }
    if (code === 0x2580) {
        ctx.fillRect(x, y, width, Math.round(height / 2));
        return true;
    }
    if (code >= 0x2589 && code <= 0x258f) {
        const part = Math.round((width * (0x2590 - code)) / 8);
        ctx.fillRect(x, y, part, height);
        return true;
    }
    if (code === 0x2590) {
        const half = Math.round(width / 2);
        ctx.fillRect(x + half, y, width - half, height);
        return true;
    }
    if (code === 0x2594) {
        ctx.fillRect(x, y, width, Math.max(1, Math.round(height / 8)));
        return true;
    }
    if (code === 0x2595) {
        const part = Math.max(1, Math.round(width / 8));
        ctx.fillRect(x + width - part, y, part, height);
        return true;
    }
    if (code >= 0x2591 && code <= 0x2593) {
        ctx.save();
        ctx.globalAlpha *= (code - 0x2590) / 4;
        ctx.fillRect(x, y, width, height);
        ctx.restore();
        return true;
    }
    if (code >= 0xe0b0 && code <= 0xe0b3) {
        ctx.beginPath();
        const right = code === 0xe0b0 || code === 0xe0b1;
        if (right) {
            ctx.moveTo(x, y);
            ctx.lineTo(x + width, y + height / 2);
            ctx.lineTo(x, y + height);
        } else {
            ctx.moveTo(x + width, y);
            ctx.lineTo(x, y + height / 2);
            ctx.lineTo(x + width, y + height);
        }
        if (code === 0xe0b0 || code === 0xe0b2) {
            ctx.closePath();
            ctx.fill();
        } else {
            ctx.save();
            ctx.strokeStyle = ctx.fillStyle;
            ctx.lineWidth = Math.max(1, Math.round(width / 8));
            ctx.stroke();
            ctx.restore();
        }
        return true;
    }
    return false;
}
