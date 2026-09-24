export type RailEdge = "start" | "end";

export const RAIL_WIDTH = {
    start: { min: 180, max: 480, initial: 258 },
    end: { min: 240, max: 560, initial: 288 },
} as const satisfies Record<RailEdge, { min: number; max: number; initial: number }>;

export function clampRailWidth(edge: RailEdge, px: number): number {
    const { min, max } = RAIL_WIDTH[edge];
    return Math.round(Math.min(max, Math.max(min, px)));
}
