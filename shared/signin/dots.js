const PITCH = 6;
const SIDE = 44;
const SAMPLES = 4;
const FIELD_PITCH = PITCH * 2;
const CLEARING = 150;
const PLAYS_FOR = 20;
const PLAYS_ON_AFTER_POINTER = 4;
const RIPPLES = 4;
const RIPPLE_LASTS = 2.4;
const STILL_TIME = 9;

const VERTEX = `#version 300 es
layout(location = 0) in vec4 dot_;
layout(location = 1) in vec3 ink;
uniform vec2 size, center, head, tail;
uniform vec3 tint, plain, ripples[${RIPPLES}];
uniform vec4 quiet[2];
uniform float density, time, heat;
uniform bool mark, failed;
out vec4 color;
flat out float edge;

vec2 away(vec2 from) { return from / max(length(from), 1.); }
float bell(float x, float width) { return exp(-x * x / (width * width)); }

float glowAt(vec2 at) {
  vec2 trail = tail - head;
  float along = clamp(dot(at - head, trail) / max(dot(trail, trail), 1.), 0., 1.);
  return heat * bell(length(at - head - trail * along), 120.) * (1. - .5 * along * min(1., length(trail) / 120.));
}

float calmAt(vec2 at) {
  float calm = 1.;
  for (int i = 0; i < 2; i++) {
    vec2 past = max(abs(at - quiet[i].xy) - quiet[i].zw, 0.);
    calm = min(calm, smoothstep(0., 70., length(past)));
  }
  return calm;
}

vec3 rippleAt(vec2 at) {
  vec3 sum = vec3(0);
  for (int i = 0; i < ${RIPPLES}; i++) {
    vec2 from = at - ripples[i].xy;
    float age = ripples[i].z;
    float front = 560. * (1. - exp(-1.7 * age));
    float hit = age < 0. ? 0. : exp(-1.9 * age) * bell(length(from) - front, 46.);
    sum += vec3(away(from) * hit, hit);
  }
  return sum;
}

void main() {
  vec2 at;
  float alpha, radius;
  if (mark) {
    float linear = clamp((time - .05 - dot_.w * .18) / .5, 0., 1.);
    float arrived = 1. - pow(1. - linear, 3.);
    vec2 home = dot_.xy * ${PITCH}.;
    alpha = arrived;
    radius = 2.55 * sqrt(dot_.z);
    if (failed) {
      float torn = max(0., (dot_.x + dot_.y) / ${SIDE}. + .28 + (dot_.w - .5) * .35);
      home += torn * torn * 150. * arrived * vec2(.5 + dot_.w, dot_.w - .2);
      alpha *= dot_.w < torn * .35 ? 0. : max(.16, 1. - torn * 1.1);
    } else {
      float turn = dot_.w * 6.28;
      home += vec2(cos(turn), sin(turn)) * (1. - arrived) * 90.;
      radius *= 1. + .1 * sin(time * 1.6 + dot_.x * .22 + dot_.y * .3);
    }
    at = center + home;
    vec2 lean = center - head, push = at - head;
    vec3 ripple = rippleAt(at);
    float near = heat * bell(length(push), 64.);
    at += heat * bell(length(lean), 260.) * away(lean) * min(length(lean) * .06, 7.);
    at += push * near * .34 + ripple.xy * 9.;
    radius *= 1. + .12 * near;
    color = vec4(ink, alpha);
  } else {
    int columns = int(ceil(size.x / ${FIELD_PITCH}.));
    at = mod(center, ${FIELD_PITCH}.) + ${FIELD_PITCH}. * vec2(gl_VertexID % columns, gl_VertexID / columns);
    float from = distance(at, center);
    float fade = max(0., 1. - from / (length(size) * .62));
    float wave = sin(from / 46. - time * 1.1 + sin(at.x / 170.) + cos(at.y / 140.));
    float lit = failed ? 0. : max(0., 1. - abs(from - (time - .35) * 900.) / 110.);
    float calm = calmAt(at);
    float glow = glowAt(at) * calm;
    vec3 ripple = rippleAt(at) * calm;
    alpha = from < ${CLEARING}. ? 0. : min(.95, fade * (.24 + .18 * wave + lit * .75) + glow * .58 + ripple.z * .6);
    radius = .8 + .5 * max(0., wave) * fade + lit * 1.1 + glow * 1.4 + ripple.z * 1.1;
    at += (at - head) * glow * .09 + ripple.xy * 7.;
    color = vec4(mix(plain, tint, smoothstep(0., .2, lit + glow + ripple.z)), alpha);
  }
  edge = radius * density;
  gl_PointSize = 2. * edge + 2.;
  gl_Position = alpha < .004 ? vec4(2, 2, 2, 1) : vec4((at / size * 2. - 1.) * vec2(1, -1), 0, 1);
}`;

const FRAGMENT = `#version 300 es
precision highp float;
in vec4 color;
flat in float edge;
out vec4 pixel;
void main() {
  float from = length(gl_PointCoord - .5) * (2. * edge + 2.);
  float alpha = color.a * clamp(edge - from + .5, 0., 1.);
  pixel = vec4(color.rgb * alpha, alpha);
}`;

function sheetOf(side) {
    const sheet = document.createElement("canvas");
    sheet.width = sheet.height = side;
    return sheet.getContext("2d", { willReadFrequently: true });
}

function channelsOf(color) {
    const ink = sheetOf(1);
    ink.fillStyle = color;
    ink.fillRect(0, 0, 1, 1);
    return [...ink.getImageData(0, 0, 1, 1).data].slice(0, 3).map((channel) => channel / 255);
}

function dotsOf(image) {
    const side = SIDE * SAMPLES;
    const ink = sheetOf(side);
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
            dots.push([
                column - SIDE / 2 + 0.5,
                row - SIDE / 2 + 0.5,
                alpha / (255 * SAMPLES * SAMPLES),
                noise - Math.floor(noise),
                ...[red, green, blue].map((channel) => channel / alpha / 255),
            ]);
        }
    return dots;
}

function moving(gl, { tint, plain, failed }) {
    const program = gl.createProgram();
    for (const [kind, source] of [
        [gl.VERTEX_SHADER, VERTEX],
        [gl.FRAGMENT_SHADER, FRAGMENT],
    ]) {
        const shader = gl.createShader(kind);
        gl.shaderSource(shader, source);
        gl.compileShader(shader);
        gl.attachShader(program, shader);
    }
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) return null;
    gl.useProgram(program);
    const to = (name) => gl.getUniformLocation(program, name);
    const at = Object.fromEntries(
        ["size", "center", "head", "tail", "density", "time", "heat", "mark", "ripples", "quiet"].map((name) => [name, to(name)]),
    );
    gl.uniform3fv(to("tint"), channelsOf(tint));
    gl.uniform3fv(to("plain"), channelsOf(plain));
    gl.uniform1i(to("failed"), failed);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);

    const field = gl.createVertexArray();
    const mark = gl.createVertexArray();
    let marked = 0;
    return {
        mark(dots) {
            marked = dots.length;
            gl.bindVertexArray(mark);
            gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
            gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(dots.flat()), gl.STATIC_DRAW);
            gl.enableVertexAttribArray(0);
            gl.vertexAttribPointer(0, 4, gl.FLOAT, false, 28, 0);
            gl.enableVertexAttribArray(1);
            gl.vertexAttribPointer(1, 3, gl.FLOAT, false, 28, 16);
        },
        place({ width, height, density, center, quiet }) {
            gl.viewport(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight);
            gl.uniform2f(at.size, width, height);
            gl.uniform2fv(at.center, center);
            gl.uniform1f(at.density, density);
            gl.uniform4fv(at.quiet, quiet);
        },
        draw({ width, height }, time, { head, tail, heat }, ripples) {
            gl.clear(gl.COLOR_BUFFER_BIT);
            gl.uniform1f(at.time, time);
            gl.uniform1f(at.heat, heat);
            gl.uniform2fv(at.head, head);
            gl.uniform2fv(at.tail, tail);
            gl.uniform3fv(at.ripples, ripples);
            gl.uniform1i(at.mark, 0);
            gl.bindVertexArray(field);
            gl.drawArrays(gl.POINTS, 0, Math.ceil(width / FIELD_PITCH) * Math.ceil(height / FIELD_PITCH));
            gl.uniform1i(at.mark, 1);
            gl.bindVertexArray(mark);
            gl.drawArrays(gl.POINTS, 0, marked);
        },
    };
}

/** For a browser without WebGL: the same picture, still. */
function still(pen, { plain, failed }) {
    let marks = [];
    const dot = (x, y, radius, alpha) => {
        pen.globalAlpha = Math.max(0, alpha);
        pen.beginPath();
        pen.arc(x, y, radius, 0, 7);
        pen.fill();
    };
    return {
        mark: (dots) => (marks = dots),
        place: ({ density }) => pen.setTransform(density, 0, 0, density, 0, 0),
        draw({ width, height, center: [centerX, centerY] }) {
            pen.clearRect(0, 0, width, height);
            pen.fillStyle = plain;
            const reach = Math.hypot(width, height) * 0.62;
            for (let y = centerY % FIELD_PITCH; y < height; y += FIELD_PITCH)
                for (let x = centerX % FIELD_PITCH; x < width; x += FIELD_PITCH) {
                    const away = Math.hypot(x - centerX, y - centerY);
                    if (away < CLEARING) continue;
                    const fade = Math.max(0, 1 - away / reach);
                    const wave = Math.sin(away / 46 - STILL_TIME * 1.1 + Math.sin(x / 170) + Math.cos(y / 140));
                    dot(x, y, 0.8 + 0.5 * Math.max(0, wave) * fade, fade * (0.24 + 0.18 * wave));
                }
            for (const [column, row, cover, chance, ...ink] of marks) {
                const torn = failed ? Math.max(0, (column + row) / SIDE + 0.28 + (chance - 0.5) * 0.35) : 0;
                if (chance < torn * 0.35) continue;
                const flung = torn * torn * 150;
                pen.fillStyle = `rgb(${ink.map((channel) => channel * 255).join(" ")})`;
                dot(
                    centerX + column * PITCH + flung * (0.5 + chance),
                    centerY + row * PITCH + flung * (chance - 0.2),
                    2.55 * Math.sqrt(cover),
                    Math.max(0.16, 1 - torn * 1.1),
                );
            }
        },
    };
}

/**
 * Draws the icon in `place` as dots gathering over a field of them, on a canvas covering the window.
 * The dots answer the pointer. Returns what stops it.
 */
export function drawDots(canvas, place, { tint, failed = false, waiting = false }) {
    const colors = { tint, failed, plain: failed ? "#8a6a70" : waiting ? "#8b8898" : tint };
    const icon = place.querySelector("svg");
    const gone = new AbortController();
    const { signal } = gone;
    const box = icon.getBBox();
    const edge = Math.max(box.width, box.height) * 1.3;
    icon.setAttribute("viewBox", `${box.x + box.width / 2 - edge / 2} ${box.y + box.height / 2 - edge / 2} ${edge} ${edge}`);

    const gl = canvas.getContext("webgl2", { antialias: false, depth: false, failIfMajorPerformanceCaveat: true });
    const start = () => (gl ? moving(gl, colors) : still(canvas.getContext("2d"), colors));
    let painter = start();
    if (!painter) {
        place.classList.add("plain");
        return () => {};
    }
    const holds = !gl || matchMedia("(prefers-reduced-motion: reduce)").matches;

    let dots = [];
    let view;
    let request = 0;
    let before = 0;
    let time = holds ? STILL_TIME : 0;
    let pace = 1;
    let playing = PLAYS_FOR;
    const ripples = [];
    const pointer = { at: [0, 0], head: [0, 0], speed: [0, 0], tail: [0, 0], heat: 0, here: false };

    const look = () => {
        const list = new Float32Array(RIPPLES * 3).fill(-1);
        ripples.forEach(({ x, y, age }, index) => list.set([x, y, age], index * 3));
        painter.draw(view, time, pointer, list);
    };

    const measure = () => {
        const density = devicePixelRatio || 1;
        const [width, height] = [canvas.clientWidth, canvas.clientHeight];
        const [across, down] = [Math.round(width * density), Math.round(height * density)];
        if (canvas.width !== across) canvas.width = across;
        if (canvas.height !== down) canvas.height = down;
        const middle = (boxes) => {
            const left = Math.min(...boxes.map((box) => box.left));
            const top = Math.min(...boxes.map((box) => box.top));
            const right = Math.max(...boxes.map((box) => box.right));
            const bottom = Math.max(...boxes.map((box) => box.bottom));
            return [(left + right) / 2, (top + bottom) / 2, (right - left) / 2, (bottom - top) / 2];
        };
        const main = place.closest("main");
        const words = [...main.children].filter((child) => !child.contains(place));
        const boxesOf = (parts) => parts.map((part) => part.getBoundingClientRect());
        view = {
            width,
            height,
            density,
            center: middle(boxesOf([place])).slice(0, 2),
            quiet: [words, [...main.nextElementSibling.children]].flatMap((parts) => middle(boxesOf(parts))),
        };
        painter.place(view);
        look();
    };

    const step = (seconds) => {
        playing = Math.max(0, playing - seconds);
        pace += ((playing ? 1 : 0) - pace) * (1 - Math.exp(-seconds * 1.5));
        time += seconds * pace;
        for (const ripple of ripples) ripple.age += seconds;
        while (ripples[0]?.age > RIPPLE_LASTS) ripples.shift();
        pointer.heat += ((pointer.here ? 1 : 0) - pointer.heat) * (1 - Math.exp(-seconds * 5));
        let left = 0;
        for (const axis of [0, 1]) {
            for (let part = 0; part < 4; part++) {
                const pull = (pointer.at[axis] - pointer.head[axis]) * 190 - pointer.speed[axis] * 21;
                pointer.speed[axis] += (pull * seconds) / 4;
                pointer.head[axis] += (pointer.speed[axis] * seconds) / 4;
            }
            pointer.tail[axis] += (pointer.head[axis] - pointer.tail[axis]) * (1 - Math.exp(-seconds * 6));
            left = Math.max(left, Math.abs(pointer.at[axis] - pointer.tail[axis]), Math.abs(pointer.speed[axis]) / 10);
        }
        const warm = pointer.here ? 1 - pointer.heat > 0.004 || left > 0.1 : pointer.heat > 0.004;
        return pace > 0.01 || ripples.length > 0 || warm;
    };

    const frame = (now) => {
        const seconds = Math.min(0.05, Math.max(0, now - before) / 1000);
        before = now;
        request = step(seconds) ? requestAnimationFrame(frame) : 0;
        look();
    };
    const wake = () => {
        if (holds || request || signal.aborted || document.hidden) return;
        before = performance.now();
        request = requestAnimationFrame(frame);
    };
    const rest = () => {
        cancelAnimationFrame(request);
        request = 0;
    };

    const listen = (target, name, heard) => target.addEventListener(name, heard, { signal, passive: true });
    const ripple = ({ clientX: x, clientY: y }) => {
        if (ripples.length === RIPPLES) ripples.shift();
        ripples.push({ x, y, age: 0 });
        wake();
    };
    const arrive = ({ clientX, clientY, pointerType }) => {
        if (pointerType === "touch") return;
        pointer.at = [clientX, clientY];
        if (pointer.heat < 0.01) Object.assign(pointer, { head: [clientX, clientY], tail: [clientX, clientY], speed: [0, 0] });
        pointer.here = true;
        playing = Math.max(playing, PLAYS_ON_AFTER_POINTER);
        wake();
    };
    const leave = () => {
        pointer.here = false;
        wake();
    };
    listen(window, "resize", measure);
    listen(window, "scroll", measure);
    document.fonts.ready.then(() => signal.aborted || measure());
    listen(document, "visibilitychange", () => (document.hidden ? rest() : wake()));
    canvas.addEventListener("webglcontextlost", (event) => event.preventDefault(), { signal });
    listen(canvas, "webglcontextrestored", () => {
        painter = start() ?? painter;
        painter.mark(dots);
        measure();
    });
    if (!holds) {
        listen(window, "pointermove", arrive);
        listen(document.documentElement, "pointerleave", leave);
        listen(window, "pointerdown", (event) => event.pointerType !== "touch" && ripple(event));
        listen(window, "pointerup", (event) => event.pointerType === "touch" && ripple(event));
    }

    measure();
    wake();

    const drawing = icon.cloneNode(true);
    drawing.setAttribute("width", SIDE * SAMPLES);
    drawing.setAttribute("height", SIDE * SAMPLES);
    const picture = new Image();
    picture.onload = () => {
        if (signal.aborted) return;
        dots = dotsOf(picture);
        painter.mark(dots);
        look();
    };
    picture.onerror = () => place.classList.add("plain");
    picture.src =
        "data:image/svg+xml," +
        encodeURIComponent(new XMLSerializer().serializeToString(drawing).replaceAll("currentColor", getComputedStyle(icon).color));
    return () => {
        gone.abort();
        rest();
    };
}
