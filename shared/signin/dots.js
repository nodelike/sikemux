const MOST_IN_MARK = 18000;
const MOST_IN_FIELD = 36000;
const FIELD_AREA_EACH = 64;
const MOST_QUIET = 8;
const PLAYS_FOR = 12;
const PLAYS_ON_AFTER_POINTER = 5;
const POINTER_FELT_FOR = 1.2;
export const SETTLES_FOR = 1.8;
const RING_LASTS = 1.8;
const MORPHS_FOR = 1.2;
export const STILL_TIME = 9;
const GLOW_CELL = 28;
const DRAWS = ["tint", "deep", "alt", "bright"];

/** Where each value of the shaders' `Scene` block starts, in floats. */
const AT = {
    size: 0,
    center: 2,
    pointer: 4,
    before: 6,
    sweep: 8,
    time: 10,
    seconds: 11,
    ring: 12,
    morph: 15,
    span: 16,
    seen: 17,
    still: 18,
    fresh: 19,
    failed: 20,
    waiting: 21,
    density: 22,
    rich: 23,
    reach: 24,
    clearing: 25,
    quiets: 26,
    tint: 28,
    glow: 31,
    deep: 32,
    across: 35,
    alt: 36,
    down: 39,
    bright: 40,
    ringTint: 44,
    quiet: 48,
};
const SCENE_FLOATS = AT.quiet + MOST_QUIET * 4;

const SCENE = `#version 300 es
precision highp float;
layout(std140) uniform Scene {
  vec2 size, center, pointer, before, sweep;
  float time, seconds;
  vec3 ring; float morph;
  float span, seen, still, fresh, failed, waiting, density, rich, reach, clearing, quiets;
  vec3 tint; float glow;
  vec3 deep; float across;
  vec3 alt; float down;
  vec3 bright, ringTint;
  vec4 quiet[${MOST_QUIET}];
};

float hash(vec2 p) { p = fract(p * vec2(123.34, 456.21)); p += dot(p, p + 45.32); return fract(p.x * p.y); }
float noise(vec2 p) {
  vec2 i = floor(p), f = fract(p);
  f = f * f * (3. - 2. * f);
  return mix(mix(hash(i), hash(i + vec2(1, 0)), f.x), mix(hash(i + vec2(0, 1)), hash(i + vec2(1, 1)), f.x), f.y);
}
`;

const SHARED = `${SCENE}
layout(location = 0) in vec4 state;
layout(location = 2) in vec4 home;
layout(location = 3) in vec2 chance;
layout(location = 6) in vec2 lookWas;
float stream(vec2 p, float t) { return noise(p + vec2(t, -t * .7)) + .5 * noise(p * 2.1 - vec2(t * 1.3, t * .4)); }
vec2 curl(vec2 p, float t) {
  const float e = .08;
  return vec2(stream(p + vec2(0, e), t) - stream(p - vec2(0, e), t), stream(p - vec2(e, 0), t) - stream(p + vec2(e, 0), t)) / (2. * e);
}

#ifdef FIELD
vec2 homeAt() {
  vec2 at = home.xy * size;
  return at + curl(at / 230., time * .035) * 14. + vec2(sin(time * .2 + chance.x * 44.), cos(time * .17 + chance.y * 51.)) * 4.;
}
vec2 onPage(vec2 at) { return at; }
#else
float torn() { return failed * max(0., home.x + home.y + .28 + (chance.x - .5) * .35); }
float turning() { return smoothstep(0., 1., (morph - chance.x * .3) / ${(MORPHS_FOR - 0.3).toFixed(1)}); }
vec2 homeAt() {
  vec2 at = home.xy * span;
  float fling = torn();
  return at + curl(at / 60., time * .05) * .5 * (1. - still) + vec2(.5 + chance.x, chance.y - .2) * fling * fling * 70.;
}
vec2 onPage(vec2 at) { return center + at; }
#endif
`;

const MOVE = `${SHARED}
out vec4 nextState;
void main() {
  gl_Position = vec4(2);
  vec2 belongs = homeAt(), at = state.xy, speed = state.zw;
#ifdef FIELD
  if (still + fresh >= 1.) {
#else
  if (still < 1. && lookWas.y < .01 && turning() < .3) {
    nextState = vec4(belongs + curl(belongs / 40., chance.x) * span * .02, 0, 0);
    return;
  }
  if (still >= 1.) {
#endif
    nextState = vec4(belongs, 0, 0);
    return;
  }
  vec2 page = onPage(at), path = before - pointer;
  vec2 off = page - pointer - path * clamp(dot(page - pointer, path) / max(dot(path, path), 1.), 0., 1.);
#ifdef FIELD
  float felt = mix(64., 134., chance.y), spring = mix(5., 11., chance.x), most = 620., leash = 200., give = .38, calm = 1.;
#else
  float felt = clamp(span * .42, 36., 98.) * mix(.8, 1.1, chance.y), spring = mix(16., 34., chance.x), give = .26, calm = turning();
  float most = clamp(span * 3.8, 320., 640.), leash = clamp(span * .56, 50., 148.);
#endif
  float far = length(off) + .01, near = seen * exp(-far * far / (felt * felt)), quick = length(sweep);
  float own = hash(chance + .71), turn = (hash(chance * 3.1 + .13) - .5) * 3.;
  vec2 fling = mat2(cos(turn), sin(turn), -sin(turn), cos(turn)) * sweep * mix(1.3, 11.5, own * own) + off / far * quick * mix(-1., 5., chance.x);
  float was = length(speed), cap = max(was, most * mix(.2, 1., own) * min(1., .3 + quick / 950.));
  speed += fling * near * calm * seconds * max(0., 1. - length(at - belongs) / leash);
  speed *= min(1., cap / max(length(speed), 1.));
  float spin = calm * seconds * ((chance.x - .5) * 14. + 5. * sin(page.x / 37. + page.y / 53. + time * 1.7));
  speed = mat2(cos(spin), sin(spin), -sin(spin), cos(spin)) * speed;
  belongs += off / far * near * felt * give;
  float drag = mix(2., 1.2, calm) * sqrt(spring);
  speed = (speed + (belongs - at) * spring * seconds) / (1. + drag * seconds);
  nextState = vec4(at + speed * seconds, speed);
}`;

const DRAW = `${SHARED}
layout(location = 4) in vec3 inkWas;
layout(location = 5) in vec3 ink;
out vec4 color;
flat out vec2 shape;
flat out float soft;
flat out float blur;

void main() {
  vec2 page = onPage(state.xy);
  float radius, alpha, lit = seen * exp(-dot(page - pointer, page - pointer) / 27000.);
  vec3 shade;
  blur = 0.;
#ifdef FIELD
  float away = length(page - center), calm = 1.;
  for (int i = 0; i < int(quiets); i++) calm *= smoothstep(0., 110., length(page - clamp(page, quiet[i].xy, quiet[i].xy + quiet[i].zw)));
  float fade = max(0., 1. - away / reach) * smoothstep(clearing - 34., clearing + 30., away);
  vec2 drift = page / 150.;
  float cloud = noise(drift + vec2(time * .025, -time * .015) + 2. * vec2(noise(drift * .7 + time * .02), noise(drift * .7 + 9. - time * .02)));
  float tone = fade * smoothstep(.12, .86, cloud) * 1.45 * rich, cut = .06 + .9 * hash(chance * 13.7);
  float shown = smoothstep(cut - .1, cut + .1, tone), core = exp(-away * away / (span * span * .55));
  float warmth = noise(page / 330. + vec2(time * .011, -time * .008) + .12 * chance.x), other = noise(page / 210. - vec2(time * .008, time * .01) + 17.);
  tone += core * .5 * step(clearing, away) * rich;
  radius = .46 + .75 * pow(chance.y, 5.) + .28 * clamp(tone, 0., 1.);
  alpha = shown * (.24 + .46 * clamp(tone, 0., 1.)) * (1. + 1. * lit + .5 * min(length(state.zw) / 160., 1.));
  alpha *= .84 + .16 * sin(time * (.2 + chance.x * .5) + chance.y * 60.) * (1. - still);
  shade = mix(deep, tint, smoothstep(.18, .62, warmth + .5 * core + .22 * (chance.x - .5)));
  shade = mix(shade, alt, smoothstep(.5, .74, other + .18 * (chance.y - .5)) * (1. - .6 * core));
  shade = mix(shade, bright, clamp(core * 1.2 * chance.x * chance.x + .5 * step(.965, chance.x) + .45 * lit * chance.y, 0., .92));
  shade = mix(mix(shade, vec3(.54, .416, .44), failed * .8), vec3(.545, .533, .596), waiting) * (.78 + .4 * chance.x);
  if (ring.z >= 0.) {
    float wave = max(0., 1. - abs(length(page - ring.xy) - ring.z * 260.) / 90.) * max(0., 1. - ring.z / ${RING_LASTS.toFixed(1)});
    shade = mix(shade, ringTint, wave);
    alpha = max(alpha, wave * .45 * step(chance.y, .4));
  }
  if (chance.x > .982) {
    radius = mix(2.4, 8.5, chance.y * chance.y);
    alpha = fade * mix(.12, .05, chance.y) * smoothstep(.1, .6, tone + .2);
    blur = radius * 1.5;
  }
  alpha *= calm;
#else
  float turned = turning(), fling = torn();
  float shine = sin(time * .5 + home.x * 7. + home.y * 9. + noise(home.xy * 6. + time * .12) * 4.);
  radius = mix(lookWas.x, home.z, turned) * max(.82, span / 430.) * 1.02 * (1. + .03 * shine);
  alpha = mix(lookWas.y, home.w, turned) * (.95 + .05 * shine);
  alpha *= chance.y < fling * .35 ? 0. : max(.16, 1. - fling * 1.1);
  shade = mix(mix(inkWas, ink, turned), vec3(1, .4, .4), .85 * failed);
  shade = mix(shade, vec3(dot(shade, vec3(.3, .59, .11))) * .75 + .1, waiting);
  shade = mix(shade, shade * 1.08, max(shine, 0.) * .6);
  shade = mix(shade, bright, .35 * lit * chance.y);
#endif
  float box = 2. * radius + 2. + blur;
  color = vec4(shade, min(alpha, 1.));
  shape = vec2(radius, box);
  soft = .55 / density;
  gl_PointSize = box * density;
  gl_Position = radius < .05 || alpha < .012 ? vec4(2, 2, 2, 1) : vec4((page / size * 2. - 1.) * vec2(1, -1), 0, 1);
}`;

const FILL = `#version 300 es
precision highp float;
in vec4 color;
flat in vec2 shape;
flat in float soft;
flat in float blur;
out vec4 pixel;
void main() {
  float past = length(gl_PointCoord - .5) * shape.y - shape.x;
  pixel = vec4(color.rgb, 1) * color.a * (blur > 0. ? pow(1. - smoothstep(-shape.x, blur * .5, past), 1.6) : 1. - smoothstep(-soft, soft, past));
}`;

// The glow is soft enough to work out at the corners of a coarse grid and blend between them.
const GLOW = `${SCENE}
out vec3 light;
float lamp(vec2 at, vec2 from, float wide) { vec2 off = at - from; return exp(-dot(off, off) / (wide * wide)); }
void main() {
  const vec2 corners[6] = vec2[](vec2(0), vec2(1, 0), vec2(0, 1), vec2(0, 1), vec2(1, 0), vec2(1));
  int cell = gl_VertexID / 6, columns = int(across);
  vec2 part = (vec2(cell % columns, cell / columns) + corners[gl_VertexID % 6]) / vec2(across, down), page = part * size;
  gl_Position = vec4((part * 2. - 1.) * vec2(1, -1), 0, 1);
  float most = length(size);
  vec2 slant = mat2(.8, -.6, .6, .8) * page;
  float cloud = noise(slant / 300. + vec2(time * .015, -time * .01) + 1.2 * vec2(noise(slant / 340. - time * .01), noise(slant.yx / 360. + 7.)));
  float second = noise(slant / 420. - vec2(time * .01, time * .015) + vec2(noise(slant / 390. + 3.), noise(slant.yx / 410.)) + 31.);
  light = (deep * .62 + tint * .16) * lamp(page, center, max(span * 1.6, most * .22)) * (.5 + cloud);
  light += tint * .3 * lamp(page, center, span * .72) * (.62 + .55 * cloud);
  light += alt * .1 * lamp(page, size * vec2(.12, 1.04), most * .46) * (.4 + 1.1 * second);
  light += deep * .22 * lamp(page, size * vec2(.98, 1.02), most * .34) * (.35 + second);
  light += tint * .14 * seen * lamp(page, pointer, 230.);
  light *= glow * (1. - .4 * waiting);
  light = mix(light, dot(light, vec3(.33)) * vec3(1.3, .6, .6), failed * .8);
}`;

const GLOW_FILL = `${SCENE}
in vec3 light;
out vec4 pixel;
void main() {
  float grain = (hash(gl_FragCoord.xy + fract(time)) - .5) * .014 * step(.004, light.r + light.g + light.b);
  vec3 lit = clamp(light + grain, 0., 1.);
  pixel = vec4(lit, max(lit.r, max(lit.g, lit.b)));
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

const mixed = (one, other, part) => one.map((channel, at) => channel + (other[at] - channel) * part);

/** The shades a lone tint is drawn in. Grey takes Sikemux's purple, so it never looks flat. */
function shadesOf(tint) {
    const [low, high] = [Math.min(...tint), Math.max(...tint)];
    if (high - low < 0.14) return { tint: mixed(tint, [0.635, 0.467, 1], 0.4), deep: [0.23, 0.17, 0.52], alt: [0.635, 0.467, 1], bright: [1, 1, 1] };
    const [red, green, blue] = tint;
    const turned = mixed(tint, [blue, red, green], 0.38);
    const [turnedLow, turnedHigh] = [Math.min(...turned), Math.max(...turned)];
    return {
        deep: tint.map((channel) => channel * channel * 0.6),
        alt: turned.map((channel) => low + ((channel - turnedLow) / (turnedHigh - turnedLow)) * (high - low)),
        bright: mixed(tint, [1, 1, 1], 0.86),
    };
}

/** A grain's colour: the look's shades for a mark of one colour, its own in light and shade for any other. */
export function inkOf({ own, lit, rim }, { tint, deep, alt, bright }) {
    if (own) {
        const raised = own.map((channel) => Math.min(1, channel * (0.74 + 0.42 * lit) + Math.max(0, lit - 0.62) * 0.22));
        return mixed(raised, alt, Math.min(0.4, rim * 0.4));
    }
    const shade = lit > 0.45 ? mixed(tint, bright, (lit - 0.45) / 0.55) : mixed(mixed(deep, tint, 0.3), tint, lit / 0.45);
    return mixed(shade, alt, Math.min(0.85, rim));
}

export function colorsOf(look) {
    const tint = channelsOf(look.tint);
    const colors = { tint, ...shadesOf(tint) };
    for (const part of DRAWS.slice(1)) if (look[part]) colors[part] = channelsOf(look[part]);
    return colors;
}

function rolls(seed) {
    return () => {
        seed = (seed + 0x6d2b79f5) | 0;
        let value = Math.imul(seed ^ (seed >>> 15), 1 | seed);
        value = (value + Math.imul(value ^ (value >>> 7), 61 | value)) ^ value;
        return ((value ^ (value >>> 14)) >>> 0) / 4294967296;
    };
}

async function pictureOf({ svg, letter, image, color = "#e7e5ef" }) {
    const side = 512;
    if (image) {
        const photo = new Image();
        photo.src = image;
        await photo.decode();
        const ink = sheetOf(side);
        ink.roundRect(0, 0, side, side, side * 0.22);
        ink.clip();
        ink.drawImage(photo, 0, 0, side, side);
        return ink.canvas;
    }
    if (letter) {
        const ink = sheetOf(side);
        const font = "600 400px Figtree, system-ui, sans-serif";
        await document.fonts.load(font);
        Object.assign(ink, { font, fillStyle: color, textAlign: "center", textBaseline: "middle" });
        ink.fillText(letter, side / 2, side / 2);
        return ink.canvas;
    }
    const drawing = new DOMParser().parseFromString(svg.replaceAll("currentColor", color), "image/svg+xml").documentElement;
    drawing.setAttribute("width", side);
    drawing.setAttribute("height", side);
    const picture = new Image();
    picture.src = "data:image/svg+xml," + encodeURIComponent(new XMLSerializer().serializeToString(drawing));
    await picture.decode();
    return picture;
}

/** Scatters grains over a picture: tightly along its outline and where the light falls, loosely in the shade. */
export async function grainsOf(mark, size, loose = 1) {
    const picture = await pictureOf(mark);
    const oneColor = mark.letter || (/currentColor/.test(mark.svg) && !/(fill|stop-color)="(#|url)/.test(mark.svg));
    const side = Math.min(420, Math.round(size * 1.6));
    const ink = sheetOf(side);
    const read = () => ink.getImageData(0, 0, side, side).data;
    ink.drawImage(picture, 0, 0, side, side);
    let pixels = read();
    let [left, top, right, bottom] = [side, side, 0, 0];
    for (let at = 0; at < side * side; at++) {
        if (pixels[at * 4 + 3] < 128) continue;
        const [x, y] = [at % side, Math.floor(at / side)];
        left = Math.min(left, x);
        right = Math.max(right, x + 1);
        top = Math.min(top, y);
        bottom = Math.max(bottom, y + 1);
    }
    if (right <= left) return [];
    const scale = (side * 0.8) / Math.max(right - left, bottom - top);
    ink.clearRect(0, 0, side, side);
    ink.drawImage(picture, side / 2 - ((left + right) / 2) * scale, side / 2 - ((top + bottom) / 2) * scale, side * scale, side * scale);
    pixels = read();

    const roll = rolls(7);
    const covered = (x, y) => x >= 0 && y >= 0 && x < side && y < side && pixels[(y * side + x) * 4 + 3] >= 128;
    const outline = [];
    const inside = [];
    for (let at = 0; at < side * side; at++) {
        const [x, y] = [at % side, Math.floor(at / side)];
        if (!covered(x, y)) continue;
        inside.push(at);
        if (!covered(x - 1, y) || !covered(x + 1, y) || !covered(x, y - 1) || !covered(x, y + 1)) outline.push(at);
    }
    for (let last = outline.length - 1; last > 0; last--) {
        const other = Math.floor(roll() * (last + 1));
        [outline[last], outline[other]] = [outline[other], outline[last]];
    }

    let gap = (Math.max(1.42, size / 300) * loose * side) / size;
    const wanted = (inside.length / (gap * gap)) * 0.62;
    const room = MOST_IN_MARK * 0.92;
    if (wanted > room) gap *= Math.sqrt(wanted / room);
    const cell = gap * 0.8;
    const across = Math.ceil(side / cell);
    const taken = new Int32Array(across * across).fill(-1);
    const grains = [];
    const drop = (at, apart, edge) => {
        const x = (at % side) + (edge ? 0.5 : roll());
        const y = Math.floor(at / side) + (edge ? 0.5 : roll());
        const [column, row] = [Math.floor(x / cell), Math.floor(y / cell)];
        if (taken[row * across + column] >= 0) return;
        const reach = Math.ceil(apart / cell);
        for (let b = Math.max(0, row - reach); b <= Math.min(across - 1, row + reach); b++)
            for (let a = Math.max(0, column - reach); a <= Math.min(across - 1, column + reach); a++) {
                const other = grains[taken[b * across + a]];
                if (other && (other.left - x) ** 2 + (other.top - y) ** 2 < apart * apart) return;
            }
        taken[row * across + column] = grains.length;
        const [u, v] = [x / side - 0.5, y / side - 0.5];
        const light = Math.max(0.55, 1.16 - 0.5 * Math.max(0, 0.42 + u * 0.55 + v * 0.95) + (roll() - 0.5) * 0.3);
        grains.push({
            left: x,
            top: y,
            x: u,
            y: v,
            radius: (edge ? 0.86 : 1) * (0.62 + 0.5 * roll() ** 2),
            light,
            own: oneColor ? null : [0, 1, 2].map((channel) => pixels[at * 4 + channel] / 255),
            lit: Math.max(0, Math.min(1, 0.52 - u * 0.62 - v * 1.02 + (roll() - 0.5) * 0.26)),
            rim: Math.max(0, (u + v) * 0.9) * (edge ? 1.25 : 0.45),
        });
    };
    for (const at of outline) drop(at, gap * 0.82, true);
    for (let tries = inside.length * 2.2; tries > 0; tries--) {
        const at = inside[Math.floor(roll() * inside.length)];
        const shade = Math.min(1, Math.max(0, 0.42 + ((at % side) / side - 0.5) * 0.55 + (Math.floor(at / side) / side - 0.5) * 0.95));
        drop(at, gap * (0.74 + 1.2 * shade * shade + 0.22 * roll()), false);
    }
    return grains;
}

const grainSize = (span) => Math.min(525, Math.max(197, Math.round(span / 33) * 33));

/** Moves and draws particles on one WebGL2 context. Null when the browser can't. */
export function particlesOn(gl, kinds) {
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    const scene = new Float32Array(SCENE_FLOATS);
    gl.bindBufferBase(gl.UNIFORM_BUFFER, 0, gl.createBuffer());
    gl.bufferData(gl.UNIFORM_BUFFER, scene.byteLength, gl.DYNAMIC_DRAW);
    const feedback = gl.createTransformFeedback();

    const link = (kind, vertex, fragment, feeds) => {
        const program = gl.createProgram();
        const define = kind === "field" ? "#version 300 es\n#define FIELD" : "#version 300 es";
        for (const [type, source] of [
            [gl.VERTEX_SHADER, vertex],
            [gl.FRAGMENT_SHADER, fragment],
        ]) {
            const shader = gl.createShader(type);
            gl.shaderSource(shader, source.replace("#version 300 es", define));
            gl.compileShader(shader);
            gl.attachShader(program, shader);
        }
        if (feeds) gl.transformFeedbackVaryings(program, ["nextState"], gl.INTERLEAVED_ATTRIBS);
        gl.linkProgram(program);
        return program;
    };
    const nothing = "#version 300 es\nprecision lowp float;\nout vec4 pixel;\nvoid main() { pixel = vec4(0); }";
    const programs = {};
    for (const kind of kinds)
        programs[kind] =
            kind === "glow" ? { draw: link(kind, GLOW, GLOW_FILL) } : { move: link(kind, MOVE, nothing, true), draw: link(kind, DRAW, FILL) };
    for (const program of Object.values(programs).flatMap(Object.values)) {
        if (!gl.getProgramParameter(program, gl.LINK_STATUS)) return null;
        gl.uniformBlockBinding(program, gl.getUniformBlockIndex(program, "Scene"), 0);
    }

    const noColumns = gl.createVertexArray();
    const hold = (data) => {
        const buffer = gl.createBuffer();
        gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
        gl.bufferData(gl.ARRAY_BUFFER, data, gl.DYNAMIC_DRAW);
        return buffer;
    };
    const point = (location, size) => {
        gl.enableVertexAttribArray(location);
        gl.vertexAttribPointer(location, size, gl.FLOAT, false, 0, 0);
    };
    return {
        set(name, ...values) {
            scene.set(values, AT[name]);
        },
        send: () => gl.bufferSubData(gl.UNIFORM_BUFFER, 0, scene),
        glow(cells) {
            gl.useProgram(programs.glow.draw);
            gl.bindVertexArray(noColumns);
            gl.drawArrays(gl.TRIANGLES, 0, cells * 6);
        },
        group(kind, count, state, columns) {
            const chance = new Float32Array(count * 2);
            const roll = rolls(3);
            for (let at = 0; at < chance.length; at++) chance[at] = roll();
            const held = [{ data: chance, size: 2, location: 3 }, ...columns].map((column) => ({ ...column, buffer: hold(column.data) }));
            const states = [hold(state), hold(state)];
            const layouts = states.map((buffer) => {
                const layout = gl.createVertexArray();
                gl.bindVertexArray(layout);
                gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
                point(0, 4);
                for (const column of held) {
                    gl.bindBuffer(gl.ARRAY_BUFFER, column.buffer);
                    point(column.location, column.size);
                }
                return layout;
            });
            gl.bindVertexArray(null);
            gl.bindBuffer(gl.ARRAY_BUFFER, null);
            let now = 0;
            return {
                update() {
                    for (const { buffer, data } of held.slice(1)) {
                        gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
                        gl.bufferSubData(gl.ARRAY_BUFFER, 0, data);
                    }
                    gl.bindBuffer(gl.ARRAY_BUFFER, null);
                },
                move(used) {
                    gl.useProgram(programs[kind].move);
                    gl.bindVertexArray(layouts[now]);
                    now = 1 - now;
                    gl.enable(gl.RASTERIZER_DISCARD);
                    gl.bindTransformFeedback(gl.TRANSFORM_FEEDBACK, feedback);
                    gl.bindBufferBase(gl.TRANSFORM_FEEDBACK_BUFFER, 0, states[now]);
                    gl.beginTransformFeedback(gl.POINTS);
                    gl.drawArrays(gl.POINTS, 0, used);
                    gl.endTransformFeedback();
                    gl.bindBufferBase(gl.TRANSFORM_FEEDBACK_BUFFER, 0, null);
                    gl.bindTransformFeedback(gl.TRANSFORM_FEEDBACK, null);
                    gl.disable(gl.RASTERIZER_DISCARD);
                },
                draw(used) {
                    gl.useProgram(programs[kind].draw);
                    gl.bindVertexArray(layouts[now]);
                    gl.drawArrays(gl.POINTS, 0, used);
                },
                drop() {
                    for (const buffer of [...states, ...held.map((column) => column.buffer)]) gl.deleteBuffer(buffer);
                    for (const layout of layouts) gl.deleteVertexArray(layout);
                },
            };
        },
    };
}

/** The particles of one mark, which turn into another mark's when shown one. */
export function markOn(particles, most) {
    const home = new Float32Array(most * 4);
    const ink = new Float32Array(most * 3);
    const inkWas = new Float32Array(most * 3);
    const lookWas = new Float32Array(most * 2);
    const group = particles.group("mark", most, new Float32Array(most * 4), [
        { data: home, size: 4, location: 2 },
        { data: inkWas, size: 3, location: 4 },
        { data: ink, size: 3, location: 5 },
        { data: lookWas, size: 2, location: 6 },
    ]);
    let [shown, before] = [0, 0];
    return {
        ...group,
        used: (morphing) => (morphing ? Math.max(shown, before) : shown),
        turnInto(grains, colors, morphed) {
            before = Math.max(shown, morphed < 1 ? before : 0);
            shown = Math.min(most, grains.length);
            const around = (grain) => Math.atan2(grain.y, grain.x) + Math.sin(grain.x * 91 + grain.y * 57) * 0.5;
            const order = grains
                .slice(0, shown)
                .map((grain) => [around(grain), grain])
                .sort(([one], [other]) => one - other);
            for (let at = 0; at < Math.max(shown, before); at++) {
                for (let part = 0; part < 2; part++) lookWas[at * 2 + part] += (home[at * 4 + 2 + part] - lookWas[at * 2 + part]) * morphed;
                for (let part = 0; part < 3; part++) inkWas[at * 3 + part] += (ink[at * 3 + part] - inkWas[at * 3 + part]) * morphed;
                if (at < shown) {
                    const grain = order[at][1];
                    home.set([grain.x, grain.y, grain.radius, grain.light], at * 4);
                    ink.set(inkOf(grain, colors), at * 3);
                } else {
                    const grain = order[at % shown]?.[1] ?? { x: 0, y: 0 };
                    home.set([grain.x, grain.y, 0, 0], at * 4);
                }
            }
            group.update();
        },
    };
}

export const toward = (value, target, rate, seconds) => value + (target - value) * (1 - Math.exp(-seconds * rate));

/** Where the pointer is, how fast it sweeps and how much of it is felt. */
export function pointerOf() {
    return {
        at: [0, 0],
        before: [0, 0],
        sweep: [0, 0],
        seen: 0,
        here: false,
        moved: -1e9,
        move(at, now) {
            if (this.here && now - this.moved < 200) {
                const seconds = Math.max(0.004, (now - this.moved) / 1000);
                const speed = at.map((value, axis) => (value - this.at[axis]) / seconds);
                const most = Math.min(1, 1500 / Math.max(1, Math.hypot(...speed)));
                this.sweep = speed.map((value, axis) => (this.sweep[axis] + value * most) / 2);
            } else this.before = at;
            Object.assign(this, { at, here: true, moved: now });
        },
        pass(now, seconds) {
            const felt = this.here && now - this.moved < POINTER_FELT_FOR * 1000;
            this.seen = toward(this.seen, +felt, felt ? 24 : 4, seconds);
            this.sweep = this.sweep.map((speed) => speed * Math.exp(-seconds * 9));
        },
    };
}

/**
 * Draws a mark as particles in a field of them, on a canvas over the page. They answer the pointer.
 * Null, and `mark` gets the class "plain", when the browser can't.
 */
export function drawDots(canvas, { mark, quiet }) {
    const gl = canvas.getContext("webgl2", { antialias: false, depth: false, failIfMajorPerformanceCaveat: true });
    let particles = gl && particlesOn(gl, ["glow", "field", "mark"]);
    if (!particles) {
        mark.classList.add("plain");
        return null;
    }
    const gone = new AbortController();
    const { signal } = gone;
    const stillness = matchMedia("(prefers-reduced-motion: reduce)");
    const field = { most: MOST_IN_FIELD, used: 0, fresh: true };
    const pointer = pointerOf();
    const ring = { since: -1e9, tint: [1, 1, 1] };
    const style = { tint: "#a277ff", tone: "worked", reach: 0.55, rich: 0.85, clearing: 0.5, glow: 0.16 };
    const goal = { colors: colorsOf(style), failed: 0, waiting: 0 };
    let mood = null;
    let view = { left: 0, top: 0, width: 0, height: 0, center: [0, 0], span: 0 };
    let [fieldGroup, markGroup] = [];
    let [shown, shownAs, grained, grains, morphedAt] = [null, "", 0, [], -1e9];
    let [request, last, until, clock, life] = [0, 0, 0, 4, 1];

    const build = () => {
        const homes = new Float32Array(MOST_IN_FIELD * 4);
        const roll = rolls(3);
        for (let at = 0; at < MOST_IN_FIELD; at++)
            homes.set([(0.5 + at * 0.7548776662 + roll() * 0.004) % 1, (0.5 + at * 0.569840291 + roll() * 0.004) % 1], at * 4);
        fieldGroup = particles.group("field", MOST_IN_FIELD, new Float32Array(MOST_IN_FIELD * 4), [{ data: homes, size: 4, location: 2 }]);
        markGroup = markOn(particles, MOST_IN_MARK);
        field.fresh = true;
    };

    const ease = (seconds) => {
        mood ??= { ...goal.colors, failed: goal.failed, waiting: goal.waiting, glow: style.glow };
        for (const part of DRAWS) mood[part] = mood[part].map((channel, at) => toward(channel, goal.colors[part][at], 3.2, seconds));
        mood.glow = toward(mood.glow, style.glow, 3.2, seconds);
        mood.failed = toward(mood.failed, goal.failed, 6, seconds);
        mood.waiting = toward(mood.waiting, goal.waiting, 6, seconds);
    };

    const paint = (now, seconds) => {
        const still = stillness.matches;
        const age = (since, lasts) => (still || now - since > lasts * 1000 ? -1 : (now - since) / 1000);
        ease(still ? 1e3 : seconds);
        const steps = seconds > 0.022 ? 2 : 1;
        const morph = still ? STILL_TIME : (now - morphedAt) / 1000;
        const { set } = particles;
        set("size", view.width, view.height, ...view.center);
        set("pointer", ...pointer.at, ...pointer.before, ...pointer.sweep);
        set("time", still ? STILL_TIME : clock, seconds / steps);
        set("ring", ...view.center, age(ring.since, RING_LASTS), morph);
        set("span", view.span, pointer.seen, +still, +field.fresh, mood.failed, mood.waiting, canvas.width / view.width);
        set("rich", style.rich, Math.hypot(view.width, view.height) * style.reach, view.span * style.clearing);
        const [across, down] = [Math.ceil(view.width / GLOW_CELL), Math.ceil(view.height / GLOW_CELL)];
        set("tint", ...mood.tint, mood.glow, ...mood.deep, across, ...mood.alt, down, ...mood.bright);
        set("ringTint", ...ring.tint);
        const marks = markGroup.used(morph < MORPHS_FOR);
        particles.send();
        for (let step = 0; step < steps; step++) {
            fieldGroup.move(field.used);
            if (marks) markGroup.move(marks);
        }
        field.fresh = false;
        pointer.before = pointer.at;
        gl.clear(gl.COLOR_BUFFER_BIT);
        if (mood.glow > 0.001) particles.glow(across * down);
        fieldGroup.draw(field.used);
        if (marks) markGroup.draw(marks);
    };

    const frame = (now) => {
        request = 0;
        const seconds = Math.min(0.05, Math.max(0.001, (now - last) / 1000));
        last = now;
        life = toward(life, until - now > SETTLES_FOR * 1000 ? 1 : 0, 2.2, seconds);
        clock += seconds * life;
        pointer.pass(now, seconds);
        paint(now, seconds);
        if (now < until) request = requestAnimationFrame(frame);
    };
    const look = () => view.width && paint(performance.now(), 0.001);
    const wake = (seconds) => {
        if (signal.aborted) return;
        if (stillness.matches) return look();
        const now = performance.now();
        until = Math.max(until, now + seconds * 1000);
        if (request || document.hidden) return;
        last = now;
        request = requestAnimationFrame(frame);
    };
    const rest = () => {
        cancelAnimationFrame(request);
        request = 0;
    };

    const measure = () => {
        if (signal.aborted) return;
        const { left, top, width, height } = canvas.getBoundingClientRect();
        if (!width || !height) return;
        const density = devicePixelRatio || 1;
        const [across, down] = [Math.round(width * density), Math.round(height * density)];
        if (canvas.width !== across || canvas.height !== down) {
            [canvas.width, canvas.height] = [across, down];
            gl.viewport(0, 0, across, down);
            field.fresh = true;
        }
        const box = mark.getBoundingClientRect();
        const center = [box.left + box.width / 2 - left, box.top + box.height / 2 - top];
        view = { left, top, width, height, center, span: box.width };
        field.used = Math.min(MOST_IN_FIELD, Math.round((width * height) / FIELD_AREA_EACH));
        const words = quiet().slice(0, MOST_QUIET);
        particles.set("quiets", words.length);
        particles.set(
            "quiet",
            ...words.flatMap((element) => {
                const range = document.createRange();
                range.selectNodeContents(element);
                const around = range.getBoundingClientRect();
                return [around.left - left, around.top - top, around.width, around.height];
            }),
        );
        if (shown && grainSize(view.span) !== grained) scatter(true);
        wake(1);
    };

    const scatter = async (instant) => {
        const [wanted, size] = [shownAs, grainSize(view.span)];
        grained = size;
        let next = [];
        let drawn = true;
        try {
            if (shown) next = await grainsOf(shown, size);
        } catch {
            drawn = false;
        }
        if (signal.aborted || wanted !== shownAs || size !== grained) return;
        mark.classList.toggle("plain", !drawn || !particles);
        grains = next;
        const now = performance.now();
        markGroup.turnInto(grains, goal.colors, Math.min(1, (now - morphedAt) / (MORPHS_FOR * 1000)));
        morphedAt = instant ? -1e9 : now;
        wake(PLAYS_FOR);
    };

    const listen = (target, name, heard) => target.addEventListener(name, heard, { signal, passive: true });
    const move = (event) => {
        pointer.move([event.clientX - view.left, event.clientY - view.top], performance.now());
        wake(PLAYS_ON_AFTER_POINTER);
    };
    listen(window, "resize", measure);
    listen(document, "scroll", measure);
    document.fonts.ready.then(measure);
    const watcher = new ResizeObserver(measure);
    for (const element of [canvas, mark]) watcher.observe(element);
    listen(document, "visibilitychange", () => (document.hidden ? rest() : wake(1)));
    listen(window, "pointermove", (event) => event.pointerType !== "touch" && move(event));
    listen(document.documentElement, "pointerleave", () => (pointer.here = false));
    listen(window, "pointerdown", (event) => {
        if (event.pointerType !== "touch") return;
        pointer.here = false;
        move(event);
    });
    listen(stillness, "change", () => wake(PLAYS_FOR));
    canvas.addEventListener("webglcontextlost", (event) => event.preventDefault(), { signal });
    listen(canvas, "webglcontextrestored", () => {
        particles = particlesOn(gl, ["glow", "field", "mark"]);
        if (!particles) return mark.classList.add("plain");
        build();
        measure();
        scatter();
    });
    build();
    measure();

    return {
        measure,
        look(change) {
            if (change.tint) for (const part of DRAWS) style[part] = undefined;
            Object.assign(style, change);
            const colors = colorsOf(style);
            const recolored = String(Object.values(colors)) !== String(Object.values(goal.colors));
            goal.colors = colors;
            goal.failed = +(style.tone === "failed");
            goal.waiting = +(style.tone === "waiting");
            if (recolored && grains.length) {
                const now = performance.now();
                markGroup.turnInto(grains, colors, Math.min(1, (now - morphedAt) / (MORPHS_FOR * 1000)));
                morphedAt = now;
            }
            if (!mood) look();
            wake(PLAYS_FOR);
        },
        show(next, { instant = false } = {}) {
            const key = JSON.stringify(next);
            if (key === shownAs) return;
            [shown, shownAs] = [next, key];
            scatter(instant);
        },
        ring(color) {
            ring.tint = channelsOf(color);
            ring.since = performance.now();
            wake(PLAYS_ON_AFTER_POINTER);
        },
        stop() {
            gone.abort();
            rest();
            watcher.disconnect();
        },
    };
}
