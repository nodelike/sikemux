export type Picture =
  { svg: string; color?: string } | { letter: string; color: string };

export type Tone = "worked" | "failed" | "waiting";

export type Look = {
  tint: string;
  deep?: string;
  bright?: string;
  tone?: Tone;
  reach?: number;
  rich?: number;
  clearing?: number;
};

export type Dots = {
  look(look: Look): void;
  show(picture: Picture | null, how?: { instant?: boolean }): void;
  ring(color: string): void;
  measure(): void;
  stop(): void;
};

export function drawDots(
  _canvas: HTMLCanvasElement,
  place: { mark: HTMLElement; quiet: () => HTMLElement[] },
): Dots | null {
  place.mark.classList.add("plain");
  return null;
}

export function drawTile(
  _canvas: HTMLCanvasElement,
  _picture: Picture,
  _place: { size: number; hot: HTMLElement },
): (() => void) | null {
  return null;
}
