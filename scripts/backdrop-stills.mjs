#!/usr/bin/env node
// Compares each backdrop kind's still frame on the Mac with the same frame drawn by the shaders
// the phones and the design page run, the clock held at 2.5 s. The Mac's frames come from the
// backdrop_stills example; the phones' GLSL runs in Chrome. Fails when they drift apart.
//
//   node scripts/backdrop-stills.mjs [--out <folder>] [--mac <folder of the example's frames>]

import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import ts from "typescript";
import { chromium } from "playwright-core";

const root = resolve(import.meta.dirname, "..");
const PICTURE = join(root, "brand/icon/icon-dark-square-1024.png");
const STILL_SECONDS = 2.5;
/** The share of compared pixels that may differ, for float rounding at a dot's edge. */
const GRAIN_BUDGET = 0.005;
/** The picture's glow and colours are baked by each client's own code, so they may differ a little. */
const PICTURE_CHANNEL_SLACK = 8;
const PICTURE_BUDGET = 0.01;

const { values: options } = parseArgs({
  options: {
    out: { type: "string", default: join(root, "target/backdrop-stills") },
    mac: { type: "string" },
  },
});
const out = resolve(options.out);
mkdirSync(out, { recursive: true });
const mac = options.mac ? resolve(options.mac) : join(out, "mac");
if (!options.mac) {
  execFileSync(
    "cargo",
    [
      "run",
      "-q",
      "-p",
      "sikemux-app",
      "--example",
      "backdrop_stills",
      "--",
      mac,
      PICTURE,
    ],
    { cwd: root, stdio: "inherit" },
  );
}

/** The phones' shader module as a plain script for the page, with the generated kinds it reads. */
function shaders() {
  const ui = join(root, "shared/backdrop");
  const plain = (file) =>
    ts
      .transpileModule(readFileSync(join(ui, file), "utf8"), {
        compilerOptions: {
          target: ts.ScriptTarget.ES2022,
          module: ts.ModuleKind.ESNext,
        },
      })
      .outputText.replace(/^import .*$/gm, "")
      .replace(/^export /gm, "");
  const names = [
    "coverVertexShader",
    "copyFragmentShader",
    "grainFragmentShader",
    "pictureFragmentShader",
    "glowMap",
    "readPicture",
  ];
  return `${plain("kinds.generated.ts")}\n${plain("shaders.ts")}\nglobalThis.backdropShaders = { ${names.join(", ")} };`;
}

const dataUrl = (file) =>
  `data:image/png;base64,${readFileSync(file).toString("base64")}`;
const inks = JSON.parse(readFileSync(join(mac, "inks.json"), "utf8"));

const browser = await chromium.launch({ channel: "chrome" });
const page = await browser.newPage();
await page.setContent("<!doctype html><title>backdrop stills</title>");
await page.addScriptTag({ content: shaders() });
const results = await page.evaluate(
  async ({ inks, picture, macGrain, macPicture, still, slack }) => {
    const {
      coverVertexShader,
      copyFragmentShader,
      grainFragmentShader,
      pictureFragmentShader,
      glowMap,
      readPicture,
    } = globalThis.backdropShaders;
    const RATIO = 2;
    const load = (src) =>
      new Promise((done, fail) => {
        const image = new Image();
        image.onload = () => done(image);
        image.onerror = fail;
        image.src = src;
      });
    const compile = (gl, type, source) => {
      const shader = gl.createShader(type);
      gl.shaderSource(shader, source);
      gl.compileShader(shader);
      if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS))
        throw new Error(gl.getShaderInfoLog(shader));
      return shader;
    };
    const link = (gl, fragment) => {
      const program = gl.createProgram();
      gl.attachShader(
        program,
        compile(gl, gl.VERTEX_SHADER, coverVertexShader),
      );
      gl.attachShader(program, compile(gl, gl.FRAGMENT_SHADER, fragment));
      gl.linkProgram(program);
      if (!gl.getProgramParameter(program, gl.LINK_STATUS))
        throw new Error(gl.getProgramInfoLog(program));
      return program;
    };
    const texture = (gl, unit) => {
      gl.activeTexture(unit);
      gl.bindTexture(gl.TEXTURE_2D, gl.createTexture());
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    };
    // Draws as the phones do and answers its pixels top row first, as a PNG keeps them.
    const draw = (width, height, fragment, uniforms, image) => {
      const canvas = document.createElement("canvas");
      canvas.width = width * RATIO;
      canvas.height = height * RATIO;
      const gl = canvas.getContext("webgl2", {
        premultipliedAlpha: true,
        preserveDrawingBuffer: true,
      });
      gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
      gl.bufferData(
        gl.ARRAY_BUFFER,
        new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]),
        gl.STATIC_DRAW,
      );
      gl.enableVertexAttribArray(0);
      gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
      if (image) {
        texture(gl, gl.TEXTURE0);
        gl.texImage2D(
          gl.TEXTURE_2D,
          0,
          gl.RGBA,
          gl.RGBA,
          gl.UNSIGNED_BYTE,
          image,
        );
        const copy = link(gl, copyFragmentShader);
        const glow = glowMap(uniforms.u_aspect, 0.4, (w, h) =>
          readPicture(gl, copy, w, h),
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
      }
      const program = link(gl, fragment);
      gl.useProgram(program);
      const all = {
        ...uniforms,
        u_resolution: [canvas.width, canvas.height],
        u_pixelRatio: RATIO,
      };
      for (const [name, value] of Object.entries(all)) {
        const at = gl.getUniformLocation(program, name);
        if (!at) continue;
        if (name === "u_image" || name === "u_glowMap") gl.uniform1i(at, value);
        else if (typeof value === "number") gl.uniform1f(at, value);
        else if (value.length === 2) gl.uniform2fv(at, value);
        else gl.uniform4fv(at, value);
      }
      gl.viewport(0, 0, canvas.width, canvas.height);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
      gl.enable(gl.BLEND);
      gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
      gl.drawArrays(gl.TRIANGLES, 0, 6);
      const flat = document.createElement("canvas");
      flat.width = canvas.width;
      flat.height = canvas.height;
      const context = flat.getContext("2d");
      context.drawImage(canvas, 0, 0);
      return {
        url: flat.toDataURL("image/png"),
        pixels: context.getImageData(0, 0, flat.width, flat.height).data,
      };
    };
    const pixelsOf = (image) => {
      const canvas = document.createElement("canvas");
      canvas.width = image.naturalWidth;
      canvas.height = image.naturalHeight;
      const context = canvas.getContext("2d");
      context.drawImage(image, 0, 0);
      return context.getImageData(0, 0, canvas.width, canvas.height).data;
    };

    const shown = await load(picture);
    const [grainMac, pictureMac] = await Promise.all([
      load(macGrain),
      load(macPicture),
    ]);
    const grain = draw(390, 422, grainFragmentShader, {
      u_ink: inks.dot.map((v, i) => (i === 3 ? 1 : v)),
      u_time: still,
    });
    const pictured = draw(
      390,
      506,
      pictureFragmentShader,
      {
        u_aspect: shown.naturalWidth / shown.naturalHeight,
        u_dotSize: 2,
        u_pattern: 8,
        u_contrast: 0,
        u_brightness: 0,
        u_glow: 0.4,
        u_ownColours: 0,
        u_full: 1,
        u_strength: 0.6,
        u_dark: [0, 0, 0, 0],
        u_middle: inks.accent,
        u_light: inks.text,
        u_image: 0,
        u_glowMap: 1,
      },
      shown,
    );

    // The Mac's frames are drawn over black, so the phones' are laid on black to compare. The Mac
    // fades the grain out by the pane's middle and rounds its corners, so only whether each dot is
    // lit is compared, in the band above that and clear of the corners.
    const onBlack = (pixels, at, channel) =>
      Math.round((pixels[at + channel] * pixels[at + 3]) / 255);
    const width = 390 * RATIO;
    const macGrainPixels = pixelsOf(grainMac);
    let grainCompared = 0;
    let grainDiffer = 0;
    for (let y = 0; y < 0.35 * 422 * RATIO; y++) {
      const corner = y < 24 * RATIO ? 24 * RATIO : 0;
      for (let x = corner; x < width - corner; x++) {
        const at = (y * width + x) * 4;
        grainCompared++;
        const lit =
          macGrainPixels[at] + macGrainPixels[at + 1] + macGrainPixels[at + 2] >
          0;
        if (lit !== grain.pixels[at + 3] > 0) grainDiffer++;
      }
    }
    const macPicturePixels = pixelsOf(pictureMac);
    let pictureCompared = 0;
    let pictureDiffer = 0;
    for (let y = 24 * RATIO; y < 506 * RATIO - 24 * RATIO; y++) {
      for (let x = 24 * RATIO; x < width - 24 * RATIO; x++) {
        const at = (y * width + x) * 4;
        pictureCompared++;
        for (let channel = 0; channel < 3; channel++) {
          if (
            Math.abs(
              macPicturePixels[at + channel] -
                onBlack(pictured.pixels, at, channel),
            ) > slack
          ) {
            pictureDiffer++;
            break;
          }
        }
      }
    }
    return {
      grain: { share: grainDiffer / grainCompared, url: grain.url },
      picture: { share: pictureDiffer / pictureCompared, url: pictured.url },
    };
  },
  {
    inks,
    picture: dataUrl(PICTURE),
    macGrain: dataUrl(join(mac, "grain.png")),
    macPicture: dataUrl(join(mac, "picture.png")),
    still: STILL_SECONDS,
    slack: PICTURE_CHANNEL_SLACK,
  },
);
await browser.close();

let failed = false;
for (const [kind, budget] of [
  ["grain", GRAIN_BUDGET],
  ["picture", PICTURE_BUDGET],
]) {
  const { share, url } = results[kind];
  writeFileSync(
    join(out, `phones-${kind}.png`),
    Buffer.from(url.slice(url.indexOf(",") + 1), "base64"),
  );
  const percent = (share * 100).toFixed(2);
  if (share > budget) {
    console.error(
      `${kind}: ${percent}% of pixels differ between the Mac and the phones, over ${(budget * 100).toFixed(2)}%`,
    );
    failed = true;
  } else {
    console.log(
      `${kind}: ${percent}% of pixels differ between the Mac and the phones`,
    );
  }
}
console.log(`Frames in ${out}`);
process.exit(failed ? 1 : 0);
