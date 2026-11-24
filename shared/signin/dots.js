const PITCH = 6;
const SIDE = 44;
const SAMPLES = 4;
const FIELD_PITCH = PITCH * 2;
const CLEARING = 150;
const PLAYS_FOR = 20;

function dotsOf(image) {
    const side = SIDE * SAMPLES;
    const sheet = document.createElement("canvas");
    sheet.width = sheet.height = side;
    const ink = sheet.getContext("2d", { willReadFrequently: true });
    ink.drawImage(image, 0, 0, side, side);
    const pixels = ink.getImageData(0, 0, side, side).data;
    const dots = [];
    for (let row = 0; row < SIDE; row++)
        for (let column = 0; column < SIDE; column++) {
            let [red, green, blue, alpha] = [0, 0, 0, 0];
            for (let sample = 0; sample < SAMPLES * SAMPLES; sample++) {
                const x = column * SAMPLES + (sample % SAMPLES);
                const y = row * SAMPLES + Math.floor(sample / SAMPLES);
                const at = (y * side + x) * 4;
                const weight = pixels[at + 3];
                red += pixels[at] * weight;
                green += pixels[at + 1] * weight;
                blue += pixels[at + 2] * weight;
                alpha += weight;
            }
            if (alpha <= 300) continue;
            const noise = Math.sin(column * 127.1 + row * 311.7) * 43758.5;
            dots.push({
                column: column - SIDE / 2 + 0.5,
                row: row - SIDE / 2 + 0.5,
                cover: alpha / (255 * SAMPLES * SAMPLES),
                color: `rgb(${(red / alpha) | 0} ${(green / alpha) | 0} ${(blue / alpha) | 0})`,
                chance: noise - Math.floor(noise),
            });
        }
    return dots;
}

/**
 * Draws the icon in `place` as dots gathering over a field of them, on a canvas covering the window.
 * Returns what stops it.
 */
export function drawDots(canvas, place, { tint, failed = false, waiting = false }) {
    const pen = canvas.getContext("2d");
    const mark = place.querySelector("svg");
    const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
    const fieldColor = failed ? "#8a6a70" : waiting ? "#8b8898" : tint;
    const gone = new AbortController();

    const box = mark.getBBox();
    const edge = Math.max(box.width, box.height) * 1.3;
    mark.setAttribute("viewBox", `${box.x + box.width / 2 - edge / 2} ${box.y + box.height / 2 - edge / 2} ${edge} ${edge}`);
    const drawing = mark.cloneNode(true);
    drawing.setAttribute("width", SIDE * SAMPLES);
    drawing.setAttribute("height", SIDE * SAMPLES);
    const picture = new Image();

    const dot = (x, y, radius) => {
        pen.beginPath();
        pen.arc(x, y, radius, 0, 7);
        pen.fill();
    };

    const drawField = (time, width, height, centerX, centerY) => {
        const ring = failed ? -1 : (time - 0.35) * 900;
        const reach = Math.hypot(width, height) * 0.62;
        for (let y = centerY % FIELD_PITCH; y < height; y += FIELD_PITCH)
            for (let x = centerX % FIELD_PITCH; x < width; x += FIELD_PITCH) {
                const away = Math.hypot(x - centerX, y - centerY);
                if (away < CLEARING) continue;
                const fade = Math.max(0, 1 - away / reach);
                const wave = Math.sin(away / 46 - time * 1.1 + Math.sin(x / 170) + Math.cos(y / 140));
                const lit = Math.max(0, 1 - Math.abs(away - ring) / 110);
                pen.globalAlpha = fade * (0.24 + 0.18 * wave) + lit * 0.75 * fade;
                pen.fillStyle = lit > 0.02 ? tint : fieldColor;
                dot(x, y, 0.8 + 0.5 * Math.max(0, wave) * fade + lit * 1.1);
            }
    };

    const drawMark = (dots, time, centerX, centerY) => {
        for (const { column, row, cover, color, chance } of dots) {
            const linear = Math.min(1, Math.max(0, (time - 0.05 - chance * 0.18) / 0.5));
            const arrived = 1 - (1 - linear) ** 3;
            let x = column * PITCH;
            let y = row * PITCH;
            let alpha = arrived;
            let radius = 2.55 * Math.sqrt(cover);
            if (failed) {
                const torn = Math.max(0, (column + row) / SIDE + 0.28 + (chance - 0.5) * 0.35);
                if (chance < torn * 0.35) continue;
                const flung = torn * torn * 150 * arrived;
                x += flung * (0.5 + chance);
                y += flung * (chance - 0.2);
                alpha *= Math.max(0.16, 1 - torn * 1.1);
            } else {
                const angle = chance * 6.28;
                const distance = (1 - arrived) * 90;
                x += Math.cos(angle) * distance;
                y += Math.sin(angle) * distance;
                radius *= 1 + 0.1 * Math.sin(time * 1.6 + column * 0.22 + row * 0.3);
            }
            pen.globalAlpha = alpha;
            pen.fillStyle = color;
            dot(centerX + x, centerY + y, radius);
        }
    };

    let settled = false;
    picture.onload = () => {
        if (gone.signal.aborted) return;
        const dots = dotsOf(picture);
        place.classList.add("drawn");
        const started = performance.now();
        const draw = (now) => {
            if (gone.signal.aborted) return;
            const time = still ? 9 : (now - started) / 1000;
            const density = devicePixelRatio || 1;
            const [width, height] = [innerWidth, innerHeight];
            if (canvas.width !== Math.round(width * density) || canvas.height !== Math.round(height * density)) {
                canvas.width = Math.round(width * density);
                canvas.height = Math.round(height * density);
            }
            pen.setTransform(density, 0, 0, density, 0, 0);
            pen.clearRect(0, 0, width, height);
            const { left, top, width: side } = place.getBoundingClientRect();
            const centerX = left + side / 2;
            const centerY = top + side / 2;
            drawField(time, width, height, centerX, centerY);
            drawMark(dots, time, centerX, centerY);
            settled = still || time >= PLAYS_FOR;
            if (!settled) requestAnimationFrame(draw);
        };
        requestAnimationFrame(draw);
        addEventListener("resize", () => settled && requestAnimationFrame(draw), {
            signal: gone.signal,
        });
    };
    picture.src =
        "data:image/svg+xml," +
        encodeURIComponent(new XMLSerializer().serializeToString(drawing).replaceAll("currentColor", getComputedStyle(mark).color));
    return () => gone.abort();
}
