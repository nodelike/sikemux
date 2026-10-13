import type { SimOrientation, SimScreen } from "../api/sim";

/** How far, in degrees clockwise, the screen turns for each way the device can be held, in the order Rotate steps through them. */
export const TURN_DEGREES: Readonly<Record<SimOrientation, number>> = {
    portrait: 0,
    landscapeRight: 90,
    portraitUpsideDown: 180,
    landscapeLeft: -90,
};

export const TURNS = Object.keys(TURN_DEGREES) as SimOrientation[];

export function turnDegrees(orientation: SimScreen["orientation"]): number {
    return Object.hasOwn(TURN_DEGREES, orientation) ? TURN_DEGREES[orientation as SimOrientation] : 0;
}
