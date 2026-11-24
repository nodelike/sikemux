export { drawDots, type Dots } from "@shared/signin/dots.js";
export { drawTile } from "@shared/signin/tiles.js";
import type { DotsMark } from "@shared/signin/dots.js";

export type Picture = DotsMark;

export type Tone = "worked" | "failed" | "waiting";
