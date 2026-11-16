import { backdropKinds } from "./kinds.generated";

/** A rectangle covering the surface; each fragment shader below works from where it is on it. */
export const coverVertexShader = `#version 300 es
layout(location = 0) in vec4 a_position;
void main() { gl_Position = a_position; }
`;

/** Draws the picture on texture unit 0 at the surface's size, its top at the first row, to read the glow from. */
export const copyFragmentShader = `#version 300 es
precision highp float;
uniform sampler2D u_image;
uniform vec2 u_resolution;
out vec4 fragColor;
void main() { fragColor = texture(u_image, gl_FragCoord.xy / u_resolution); }
`;

/** The Mac's grain in one ink, laid out from the surface's middle in points. */
export const grainFragmentShader = `#version 300 es
${backdropKinds}
uniform vec2 u_resolution;
uniform float u_pixelRatio;
uniform float u_time;
uniform vec4 u_ink;
out vec4 fragColor;

void main() {
  float lit = grain((gl_FragCoord.xy - 0.5 * u_resolution) / u_pixelRatio, u_time);
  fragColor = vec4(u_ink.rgb * u_ink.a * lit, u_ink.a * lit);
}
`;

/** The Mac's picture kind over the picture on texture unit 0 and its glow on unit 1. */
export const pictureFragmentShader = `#version 300 es
${backdropKinds}
uniform sampler2D u_image;
uniform sampler2D u_glowMap;
uniform vec2 u_resolution;
uniform float u_pixelRatio;
uniform float u_aspect;
uniform float u_dotSize;
uniform float u_pattern;
uniform float u_contrast;
uniform float u_brightness;
uniform float u_glow;
uniform float u_ownColours;
uniform float u_full;
uniform float u_strength;
uniform vec4 u_dark;
uniform vec4 u_middle;
uniform vec4 u_light;
out vec4 fragColor;

void main() {
  vec2 size = u_resolution / u_pixelRatio;
  vec2 point = gl_FragCoord.xy / u_pixelRatio - 0.5 * size;
  vec3 texel = texture(u_image, picture_cover(picture_dot(point, u_dotSize), size, u_aspect)).rgb;
  float glowing = texture(u_glowMap, picture_cover(point, size, u_aspect)).r;
  float shown = (u_full > 0.5 ? 1.0 : picture_fade(1.0 - gl_FragCoord.y / u_resolution.y)) * u_strength;
  fragColor = picture(point, texel, glowing, u_dotSize, u_pattern, u_contrast, u_brightness, u_glow, u_ownColours,
    u_dark, u_middle, u_light, shown);
}
`;

const GLOW_LONGEST = 320;
/** How bright a place must be, from 0 to 1, before it glows. */
const GLOW_THRESHOLD = 0.45;

/**
 * How much each place of the picture glows, as the Mac works it out: its bright parts on a small copy, blurred near and
 * far by `glowSize`. `read` gives the picture's pixels at a size, top row first.
 */
export function glowMap(
    aspect: number,
    glowSize: number,
    read: (width: number, height: number) => Uint8Array,
): { width: number; height: number; pixels: Uint8Array } {
    const width = Math.max(Math.round(aspect >= 1 ? GLOW_LONGEST : GLOW_LONGEST * aspect), 1);
    const height = Math.max(Math.round(aspect >= 1 ? GLOW_LONGEST / aspect : GLOW_LONGEST), 1);
    const picture = read(width, height);
    const bright = new Uint8Array(width * height);
    for (let at = 0; at < bright.length; at++) {
        const lum = (0.2126 * picture[at * 4] + 0.7152 * picture[at * 4 + 1] + 0.0722 * picture[at * 4 + 2]) / 255;
        bright[at] = Math.floor(Math.max((lum - GLOW_THRESHOLD) / (1 - GLOW_THRESHOLD), 0) * 255);
    }
    const spread = 1 + glowSize * 10;
    const near = blurred(bright, width, height, spread);
    const far = blurred(bright, width, height, spread * 3);
    const pixels = new Uint8Array(width * height * 4);
    for (let at = 0; at < bright.length; at++) {
        const glow = Math.floor(Math.min(near[at] * 1.2 + far[at] * 0.8, 255));
        pixels.set([glow, glow, glow, 255], at * 4);
    }
    return { width, height, pixels };
}

/** A Gaussian blur made of three box blurs, as the Mac's `fast_blur` makes it. */
function blurred(pixels: Uint8Array, width: number, height: number, sigma: number): Uint8Array {
    const passes = 3;
    let lower = Math.floor(Math.sqrt((12 * sigma * sigma) / passes + 1));
    if (lower % 2 === 0) lower -= 1;
    const upper = lower + 2;
    const split = Math.max(Math.round(0.25 * passes * (lower + 3) - (3 * sigma * sigma) / (lower + 1)), 0);
    let out = pixels;
    for (let pass = 0; pass < passes; pass++) {
        const half = Math.floor(Math.max((pass < split ? lower : upper) - 1, 0) / 2);
        const radius = half % 2 === 0 ? half + 1 : half;
        const across = box(out, height, width, 1, width, radius);
        out = box(across, width, height, width, 1, radius);
    }
    return out;
}

/** Averages each run of `length` pixels over `radius` either side, holding the run's end pixels past its ends. */
function box(source: Uint8Array, count: number, length: number, step: number, stride: number, radius: number) {
    const target = new Uint8Array(source.length);
    const span = 2 * radius + 1;
    for (let run = 0; run < count; run++) {
        const first = run * stride;
        const at = (index: number) => source[first + Math.min(Math.max(index, 0), length - 1) * step];
        let sum = 0;
        for (let index = -radius; index <= radius; index++) sum += at(index);
        for (let index = 0; index < length; index++) {
            target[first + index * step] = Math.round(sum / span);
            sum += at(index + radius + 1) - at(index - radius);
        }
    }
    return target;
}

/** Reads the picture bound to texture unit 0 back at a small size, top row first. */
export function readPicture(gl: WebGL2RenderingContext, program: WebGLProgram, width: number, height: number): Uint8Array {
    const target = gl.createTexture();
    gl.activeTexture(gl.TEXTURE2);
    gl.bindTexture(gl.TEXTURE_2D, target);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, width, height, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    const frame = gl.createFramebuffer();
    gl.bindFramebuffer(gl.FRAMEBUFFER, frame);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, target, 0);
    gl.useProgram(program);
    gl.uniform1i(gl.getUniformLocation(program, "u_image"), 0);
    gl.uniform2fv(gl.getUniformLocation(program, "u_resolution"), [width, height]);
    gl.viewport(0, 0, width, height);
    gl.disable(gl.BLEND);
    gl.drawArrays(gl.TRIANGLES, 0, 6);
    const pixels = new Uint8Array(width * height * 4);
    gl.readPixels(0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    gl.deleteFramebuffer(frame);
    gl.deleteTexture(target);
    gl.activeTexture(gl.TEXTURE0);
    return pixels;
}
