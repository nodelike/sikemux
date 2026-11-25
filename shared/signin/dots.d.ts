export type DotsMark = { svg: string; color?: string } | { letter: string; color: string } | { image: string };

/** A lone tint brings shades of its own. */
export interface DotsColors {
    tint: string;
    deep?: string;
    alt?: string;
    bright?: string;
}

export interface Dots {
    look(
        change: Partial<DotsColors> & { tone?: "worked" | "failed" | "waiting"; glow?: number; reach?: number; rich?: number; clearing?: number },
    ): void;
    show(mark: DotsMark | null, how?: { instant?: boolean }): void;
    ring(color: string): void;
    /** Only when the layout moved without a resize or a scroll. */
    measure(): void;
    stop(): void;
}

export function drawDots(canvas: HTMLCanvasElement, place: { mark: HTMLElement; quiet: () => HTMLElement[] }): Dots | null;
