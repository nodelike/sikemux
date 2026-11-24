import type { DotsColors, DotsMark } from "./dots.js";

/** `size` is the mark's in CSS pixels; the canvas reaches past it on every side without taking room in the layout. */
export function drawTile(canvas: HTMLCanvasElement, mark: DotsMark, how: { size: number; hot: HTMLElement; look?: DotsColors }): (() => void) | null;
