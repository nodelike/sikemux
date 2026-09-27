// JetBrains Mono's ascender plus descender, as a fraction of the font size.
// Ghostty spaces rows by this, so a row there is 1.32 times the font size.
const FONT_LINE_HEIGHT_EM = 1.32;

// WebGL rounds the cell down to a whole screen pixel, so letters 7.8 pixels
// wide get a 7.5 pixel cell and the line looks squeezed. letterSpacing is added
// to that rounded-down width, so one pixel back lands the cell on whichever
// whole pixel is nearest the real width.
export function cellWidthCorrection(charWidth: number, devicePixelRatio: number): number {
    if (!Number.isFinite(charWidth) || charWidth <= 0) return 0;
    if (!Number.isFinite(devicePixelRatio) || devicePixelRatio <= 0) return 0;
    const exact = charWidth * devicePixelRatio;
    return Math.round(exact) - Math.floor(exact);
}

// WebKit reports 15 pixels for a 13 pixel line of JetBrains Mono where the font
// itself asks for 17.16, and xterm sizes rows from that report, so rows come out
// short and the text looks stretched sideways. This is the lineHeight that
// brings a row back to the font's own height, on a whole screen pixel.
export function lineHeightCorrection(fontSize: number, charHeight: number, devicePixelRatio: number): number {
    if (!Number.isFinite(fontSize) || fontSize <= 0) return 1;
    if (!Number.isFinite(charHeight) || charHeight <= 0) return 1;
    if (!Number.isFinite(devicePixelRatio) || devicePixelRatio <= 0) return 1;
    const reported = Math.ceil(charHeight * devicePixelRatio);
    const wanted = Math.round(fontSize * FONT_LINE_HEIGHT_EM * devicePixelRatio);
    if (wanted <= reported) return 1;
    // xterm floors reported * lineHeight, so aim half a pixel past the target.
    return (wanted + 0.5) / reported;
}

export interface CharSize {
    width: number;
    height: number;
}

const measured = new Map<string, CharSize>();

// Measured the way xterm measures it, so the correction matches what it rounds.
export function measureChar(fontFamily: string, fontSize: number): CharSize {
    const font = `${fontSize}px ${fontFamily}`;
    const cached = measured.get(font);
    if (cached !== undefined) return cached;
    let size = { width: 0, height: 0 };
    try {
        const ctx = new OffscreenCanvas(100, 100).getContext("2d");
        if (ctx) {
            ctx.font = font;
            const metrics = ctx.measureText("W");
            size = { width: metrics.width, height: metrics.fontBoundingBoxAscent + metrics.fontBoundingBoxDescent };
        }
    } catch {
        size = { width: 0, height: 0 };
    }
    // A terminal that booted before the font arrived would cache the fallback.
    if (size.width > 0) measured.set(font, size);
    return size;
}
