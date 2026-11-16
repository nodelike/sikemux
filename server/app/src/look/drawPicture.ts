import { getShaderColorFromString } from "@paper-design/shaders";
import {
  copyFragmentShader,
  coverVertexShader,
  glowMap,
  pictureFragmentShader,
  readPicture,
} from "@shared/backdrop/shaders";
import type { BackdropLook } from "@shared/wire.generated";

/** As on the Mac and the phone: the picture shows at 60% of the look's strength. */
const PICTURE_OPACITY = 0.6;

type Uniform = number | number[];

function uniforms(look: BackdropLook, aspect: number): Record<string, Uniform> {
  const style = getComputedStyle(document.documentElement);
  const fallback = [
    undefined,
    style.getPropertyValue("--acc"),
    style.getPropertyValue("--ink"),
  ];
  const ink = (at: number): number[] => {
    const value = look.inks.length ? look.inks[at] : fallback[at];
    if (!value) return [0, 0, 0, 0];
    const [r, g, b] = getShaderColorFromString(value.trim());
    return [r, g, b, 1];
  };
  return {
    u_aspect: aspect,
    u_dotSize: look.dotSize,
    u_pattern: { bayer2: 2, bayer4: 4, bayer8: 8, noise: 0 }[look.pattern] ?? 8,
    u_contrast: look.contrast,
    u_brightness: look.brightness,
    u_glow: look.glow,
    u_ownColours: look.ownColours ? 1 : 0,
    u_full: look.coverage === "full" ? 1 : 0,
    u_strength: Math.min(PICTURE_OPACITY * look.strength, 1),
    u_dark: ink(0),
    u_middle: ink(1),
    u_light: ink(2),
  };
}

function link(
  gl: WebGL2RenderingContext,
  fragment: string,
): WebGLProgram | null {
  const program = gl.createProgram();
  for (const [type, source] of [
    [gl.VERTEX_SHADER, coverVertexShader],
    [gl.FRAGMENT_SHADER, fragment],
  ] as const) {
    const shader = gl.createShader(type);
    if (!shader) return null;
    gl.shaderSource(shader, source);
    gl.compileShader(shader);
    gl.attachShader(program, shader);
  }
  gl.linkProgram(program);
  return gl.getProgramParameter(program, gl.LINK_STATUS) ? program : null;
}

function texture(gl: WebGL2RenderingContext, unit: number) {
  gl.activeTexture(unit);
  gl.bindTexture(gl.TEXTURE_2D, gl.createTexture());
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
}

/**
 * Draws the host's picture dithered in its look on a canvas in `element`, as the Mac and the phone draw it,
 * and redraws it when the element changes size. Resolves to what takes it away, or null without WebGL 2.
 */
export async function drawPicture(
  element: HTMLElement,
  picture: string,
  look: BackdropLook,
): Promise<(() => void) | null> {
  const image = new Image();
  image.src = picture;
  await image.decode();
  const canvas = document.createElement("canvas");
  const gl = canvas.getContext("webgl2", {
    antialias: false,
    premultipliedAlpha: true,
  });
  if (!gl) return null;
  const program = link(gl, pictureFragmentShader);
  const copy = link(gl, copyFragmentShader);
  if (!program || !copy) return null;
  const aspect = image.naturalWidth / image.naturalHeight;

  gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
  gl.bufferData(
    gl.ARRAY_BUFFER,
    new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]),
    gl.STATIC_DRAW,
  );
  gl.enableVertexAttribArray(0);
  gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);

  texture(gl, gl.TEXTURE0);
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, image);
  const glow = glowMap(aspect, look.glowSize, (width, height) =>
    readPicture(gl, copy, width, height),
  );
  texture(gl, gl.TEXTURE1);
  gl.texImage2D(
    gl.TEXTURE_2D,
    0,
    gl.RGBA,
    glow.width,
    glow.height,
    0,
    gl.RGBA,
    gl.UNSIGNED_BYTE,
    glow.pixels,
  );
  gl.activeTexture(gl.TEXTURE0);
  gl.deleteProgram(copy);

  gl.useProgram(program);
  gl.uniform1i(gl.getUniformLocation(program, "u_image"), 0);
  gl.uniform1i(gl.getUniformLocation(program, "u_glowMap"), 1);
  for (const [name, value] of Object.entries(uniforms(look, aspect))) {
    const at = gl.getUniformLocation(program, name);
    if (typeof value === "number") gl.uniform1f(at, value);
    else gl.uniform4fv(at, value);
  }

  const draw = () => {
    const ratio = devicePixelRatio;
    canvas.width = Math.max(Math.round(element.clientWidth * ratio), 1);
    canvas.height = Math.max(Math.round(element.clientHeight * ratio), 1);
    gl.viewport(0, 0, canvas.width, canvas.height);
    gl.uniform2fv(gl.getUniformLocation(program, "u_resolution"), [
      canvas.width,
      canvas.height,
    ]);
    gl.uniform1f(gl.getUniformLocation(program, "u_pixelRatio"), ratio);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.drawArrays(gl.TRIANGLES, 0, 6);
  };
  draw();
  const resized = new ResizeObserver(draw);
  resized.observe(element);
  element.append(canvas);
  return () => {
    resized.disconnect();
    canvas.remove();
    gl.getExtension("WEBGL_lose_context")?.loseContext();
  };
}
