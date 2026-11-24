import { colorsOf, grainsOf, markOn, particlesOn, pointerOf, SETTLES_FOR, STILL_TIME, toward } from "./dots.js";

const MOVES_FOR = 3.4;
const ROOM_AROUND = 32;
/** A tile's particles move as a mark this much larger would, so the pointer scatters part of the mark and not all of it. */
const LARGER = 2.4;
const GRAINS_SIZE = 192;
const GRAINS_LOOSE = 1.4;
const GRAINS_LARGER = 2.2;

const scattered = new Map();
const moving = new Set();
let queue = Promise.resolve();
let stage = null;
let request = 0;

/** The one WebGL canvas every tile is drawn on before it's copied to its own. */
function stageOf() {
    if (stage && !stage.gl.isContextLost()) return stage;
    const canvas = document.createElement("canvas");
    const gl = canvas.getContext("webgl2", { antialias: false, depth: false, failIfMajorPerformanceCaveat: true });
    const particles = gl && particlesOn(gl, ["mark"]);
    stage = particles ? { canvas, gl, particles } : null;
    return stage;
}

function grainsFor(mark) {
    const key = JSON.stringify(mark);
    if (!scattered.has(key))
        scattered.set(
            key,
            (queue = queue
                .then(() => new Promise(requestAnimationFrame))
                .then(() => grainsOf(mark, GRAINS_SIZE, GRAINS_LOOSE))
                .then(
                    (grains) => grains.map((grain) => ({ ...grain, radius: grain.radius * GRAINS_LARGER })),
                    () => [],
                )),
        );
    return scattered.get(key);
}

function frame(now) {
    request = 0;
    for (const step of [...moving]) step(now);
    if (moving.size) request = requestAnimationFrame(frame);
}

/** Draws a mark as a small picture of particles that answer the pointer while it's over `hot`. Null when the browser can't. */
export function drawTile(canvas, mark, { size, hot, look = { tint: "#a277ff" } }) {
    if (!stageOf()) return null;
    const ink = canvas.getContext("2d");
    const gone = new AbortController();
    const { signal } = gone;
    const stillness = matchMedia("(prefers-reduced-motion: reduce)");
    const colors = colorsOf(look);
    const pointer = pointerOf();
    const side = size + ROOM_AROUND * 2;
    let grains = [];
    let [group, on, until, last, clock, life] = [null, null, 0, 0, STILL_TIME, 0];
    Object.assign(canvas.style, { width: `${side}px`, height: `${side}px`, margin: `${-ROOM_AROUND}px`, pointerEvents: "none" });

    const gather = () => {
        on = stageOf();
        group = on && markOn(on.particles, grains.length, false);
        group?.turnInto(grains, colors, 1);
        return group;
    };
    const letGo = () => {
        group?.drop();
        group = null;
    };
    const paint = (seconds) => {
        const pixels = Math.round(side * (devicePixelRatio || 1));
        if (canvas.width !== pixels) canvas.width = canvas.height = pixels;
        const { gl, particles } = on;
        if (on.canvas.width < pixels) on.canvas.width = on.canvas.height = pixels;
        const { set } = particles;
        const wide = side * LARGER;
        set("size", wide, wide, wide / 2, wide / 2);
        set("pointer", ...pointer.at, ...pointer.before, ...pointer.sweep);
        set("time", clock, seconds, 0, 0, -1);
        set("ring", 0, 0, -1, STILL_TIME);
        set("span", size * LARGER, pointer.seen, seconds ? 1 - life : 1, 0, 0, 0, pixels / wide);
        set("quiets", 0);
        set("tint", ...colors.tint, 0, ...colors.deep, 0, ...colors.alt, 0, ...colors.bright);
        particles.send();
        group.move(grains.length);
        pointer.before = pointer.at;
        gl.viewport(0, 0, pixels, pixels);
        gl.clear(gl.COLOR_BUFFER_BIT);
        group.draw(grains.length);
        ink.clearRect(0, 0, pixels, pixels);
        ink.drawImage(on.canvas, 0, on.canvas.height - pixels, pixels, pixels, 0, 0, pixels, pixels);
    };
    const still = () => {
        if (!grains.length || moving.has(step) || !gather()) return;
        paint(0);
        letGo();
    };
    const step = (now) => {
        const seconds = Math.min(0.022, Math.max(0.001, (now - last) / 1000));
        last = now;
        life = toward(life, until - now > SETTLES_FOR * 1000 ? 0.999 : 0.001, 3.2, seconds);
        clock += seconds * life;
        pointer.pass(now, seconds);
        if (on?.gl.isContextLost()) group = null;
        else paint(now < until ? seconds : 0);
        if (group && now < until) return;
        moving.delete(step);
        letGo();
    };
    const move = (event) => {
        if (event.pointerType === "touch" || stillness.matches || !grains.length) return;
        const box = canvas.getBoundingClientRect();
        const now = performance.now();
        pointer.move([(event.clientX - box.left) * LARGER, (event.clientY - box.top) * LARGER], now);
        until = now + MOVES_FOR * 1000;
        if (moving.has(step) || !gather()) return;
        last = now;
        life = 0.001;
        paint(0);
        moving.add(step);
        request ||= requestAnimationFrame(frame);
    };
    hot.addEventListener("pointermove", move, { signal, passive: true });
    hot.addEventListener("pointerleave", () => (pointer.here = false), { signal, passive: true });
    addEventListener("resize", () => canvas.width !== Math.round(side * (devicePixelRatio || 1)) && still(), { signal, passive: true });
    grainsFor(mark).then((found) => {
        if (signal.aborted) return;
        grains = found;
        still();
    });

    return () => {
        gone.abort();
        moving.delete(step);
        letGo();
    };
}
