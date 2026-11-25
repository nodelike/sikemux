import { initWasm, Resvg } from "@resvg/resvg-wasm";
import wasm from "@resvg/resvg-wasm/index_bg.wasm";

import markSvg from "../../../../../brand/mark/mark-white.svg";
import lockupSvg from "../../../../../brand/wordmark/lockup-white.svg";
import regular from "../../../../../helpers/notch/Fonts/Figtree_400Regular.ttf";
import medium from "../../../../../helpers/notch/Fonts/Figtree_500Medium.ttf";
import semibold from "../../../../../helpers/notch/Fonts/Figtree_600SemiBold.ttf";

import { Font } from "./font.ts";
import { layTitle, LINE_HEIGHT, shorten, TITLE_SPACING } from "./layout.ts";
import { drawNote, HEIGHT, WIDTH } from "./note.ts";
import { encodePng } from "./png.ts";

export interface Card {
  id: string;
  title: string;
  sharedBy: string | null;
}

const LEFT = 70;
const TITLE = { width: 530, height: 340, sizes: [92, 76, 64, 54, 46] };
const TITLE_MIDDLE = 330;
const BY_LINE = 562;
const BY_SIZE = 27;
const NAME_WIDTH = 300;
const INK = "#fff";
const NAME_INK = "#eeecf4";
const KIND_INK = "#aeabbb";
const KIND = "Artifact";

const fonts = {
  400: new Font(regular),
  500: new Font(medium),
  600: new Font(semibold),
};

const escape = (text: string) =>
  text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

const text = (
  words: string,
  x: number,
  y: number,
  size: number,
  weight: keyof typeof fonts,
  ink: string,
  more = "",
) =>
  `<text x="${x}" y="${y}" font-family="Figtree" font-size="${size}" font-weight="${weight}" fill="${ink}" ${more}>${escape(words)}</text>`;

function place(svg: string, x: number, y: number, height: number): string {
  const box = /viewBox="([\d. ]+)"/.exec(svg)?.[1] ?? "0 0 1 1";
  const [, , wide = 1, tall = 1] = box.split(" ").map(Number);
  const drawing = svg.replace(/<\?xml[^>]*>|<\/?svg[^>]*>/g, "");
  return `<svg x="${x}" y="${y}" width="${(wide / tall) * height}" height="${height}" viewBox="${box}">${drawing}</svg>`;
}

function titleLines(title: string): string {
  const { size, lines } = layTitle(fonts[600], title, TITLE);
  const apart = size * LINE_HEIGHT;
  const first = TITLE_MIDDLE - ((lines.length - 1) * apart) / 2 + size * 0.3;
  const spacing = `letter-spacing="${size * TITLE_SPACING}"`;
  return lines
    .map((line, i) =>
      text(line, LEFT, first + i * apart, size, 600, INK, spacing),
    )
    .join("");
}

function byLine(sharedBy: string | null): string {
  const drawn = sharedBy && fonts[500].drawable(sharedBy);
  if (!drawn) return text(KIND, LEFT, BY_LINE, BY_SIZE, 400, KIND_INK);
  const name = shorten(fonts[500], drawn, BY_SIZE, NAME_WIDTH);
  const initial = (Array.from(name)[0] ?? "").toUpperCase();
  const after = 124 + fonts[500].width(name, BY_SIZE) + 14;
  return [
    `<circle cx="91" cy="${BY_LINE - 9}" r="19" fill="#23232a"/>`,
    text(
      initial,
      91,
      BY_LINE - 1.5,
      21,
      600,
      "#cdb6ff",
      'text-anchor="middle"',
    ),
    text(name, 124, BY_LINE, BY_SIZE, 500, NAME_INK),
    text(`·  ${KIND}`, after, BY_LINE, BY_SIZE, 400, KIND_INK),
  ].join("");
}

/** What stands over the figure, and where the figure is: with the mark alone when the title is in letters Figtree lacks. */
function compose({ title, sharedBy }: Card) {
  const drawn = fonts[600].drawable(title);
  if (!drawn)
    return {
      over: place(markSvg, 554, 269, 92),
      at: { x: WIDTH / 2, y: HEIGHT / 2, radius: 285 },
    };
  return {
    over:
      place(markSvg, 856, 271, 88) +
      place(lockupSvg, LEFT, 60, 36) +
      titleLines(drawn) +
      byLine(sharedBy),
    at: { x: 900, y: HEIGHT / 2, radius: 270 },
  };
}

/** Lays `over`, whose colours are already multiplied by their alpha, on the figure. */
function flatten(under: Float32Array, over: Uint8Array): Uint8Array {
  const out = new Uint8Array(WIDTH * HEIGHT * 3);
  for (let i = 0; i < WIDTH * HEIGHT; i++) {
    const through = 1 - (over[i * 4 + 3] ?? 0) / 255;
    for (let k = 0; k < 3; k++)
      out[i * 3 + k] = Math.min(
        255,
        Math.max(0, under[i * 3 + k] ?? 0) * through +
          (over[i * 4 + k] ?? 0) +
          0.5,
      );
  }
  return out;
}

const ready = initWasm(wasm);

export async function drawCard(card: Card): Promise<Buffer> {
  await ready;
  const { over, at } = compose(card);
  const svg = new Resvg(
    `<svg xmlns="http://www.w3.org/2000/svg" width="${WIDTH}" height="${HEIGHT}" viewBox="0 0 ${WIDTH} ${HEIGHT}">${over}</svg>`,
    {
      font: {
        fontBuffers: [regular, medium, semibold],
        defaultFontFamily: "Figtree",
      },
    },
  );
  const image = svg.render();
  try {
    return encodePng(
      flatten(drawNote(card.id, at), image.pixels),
      WIDTH,
      HEIGHT,
    );
  } finally {
    image.free();
    svg.free();
  }
}
