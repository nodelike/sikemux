/**
 * A swipe ends when the hand leaves and whatever glide it left has run out.
 * Nothing in the window says when that is: the same silence covers a hand
 * resting part way through a swipe and a hand that has gone. macOS knows which,
 * so the wait is short once it says the hand has lifted, and long enough while
 * it is still down that only a hand left there for seconds ends the swipe.
 */
export const HELD_END_MS = 3000;
/** Long enough for the next of the events a swipe glides out on, and no longer. */
export const SPENT_END_MS = 100;
/**
 * And where nothing is watching the trackpad, the two are back to looking alike
 * and no wait suits both. Long enough to sit through a short pause, short enough
 * that a swipe already over does not hang on.
 */
export const UNWATCHED_END_MS = 600;

/** How long to let the quiet run before a swipe counts as finished. */
export const endDelay = (fingersDown: boolean | null) => (fingersDown === null ? UNWATCHED_END_MS : fingersDown ? HELD_END_MS : SPENT_END_MS);
/** Only the last moments of a swipe say anything about how it ended. */
const FLICK_WINDOW_MS = 90;
/**
 * How far a swipe has to have moved in those moments to count as thrown rather
 * than placed. In pixels of hand, not screens: a hand does not move further
 * because the screen it is on is wider, so asking it to would make the same
 * throw work on a laptop and fail on a big monitor. Whatever the pointer speed
 * the machine is set to is already in these, which is why it is what it is.
 */
const FLICK_TRAVEL_PX = 70;
/**
 * How far a pull has to have got before letting go of it takes the screen it was
 * heading for. Less than the half a screen a pull crosses on: the crossing moves
 * the session while a hand is still on the track, so it has to be sure, while a
 * hand that has gone is not going to pull any further and what it did is all
 * there is to go on.
 */
const COMMIT = 0.3;

/** How far one wheel event pushed the track, and when. */
export interface Push {
    readonly at: number;
    readonly pixels: number;
}

/** The pushes from the last moments of a swipe, with anything older dropped. */
export function pushed(pushes: readonly Push[], at: number, pixels: number): Push[] {
    const recent = pushes.filter((push) => at - push.at <= FLICK_WINDOW_MS);
    recent.push({ at, pixels });
    return recent;
}

/** Which way the hand was still going when the swipe ended, and how hard, in pixels. */
export function thrust(pushes: readonly Push[], until: number): number {
    let moved = 0;
    for (const push of pushes) if (until - push.at <= FLICK_WINDOW_MS) moved += push.pixels;
    return moved;
}

/** How many screens of follow-through a swipe had left in it: -1, 0 or 1. */
export function flicked(pushes: readonly Push[], until: number): number {
    const moved = thrust(pushes, until);
    return Math.abs(moved) < FLICK_TRAVEL_PX ? 0 : Math.sign(moved);
}

/**
 * Whether a swipe set down rather than thrown has still pulled far enough to
 * take the screen it was heading for: -1, 0 or 1.
 *
 * `way` is the way the hand was last going, and the offset alone cannot stand in
 * for it. A pull that has crossed onto a screen is counted from that screen and
 * so sits behind it, a long way off it and still going forwards — reading that
 * as a pull backwards would hand the session back to the screen just left.
 */
export const pulledOn = (offset: number, way: number) => (Math.abs(offset) >= COMMIT && Math.sign(offset) === way ? way : 0);

/** Below this the gesture is diagonal enough to belong to whatever is under it. */
const HORIZONTAL_RATIO = 1.5;
/** How far a gesture can pull past the first or last screen of the session. */
const OVERSCROLL = 0.15;
/** How much of the pull gets through before the resistance takes over. */
const GIVE = 0.55;
/**
 * How far the track has to slide off a screen before the one arriving is the one
 * you are on. Half a screen, because the screen it leaves behind is then half a
 * screen the other way, and anything closer would hand the session back and
 * forth across a single threshold.
 */
const HANDOVER = 0.5;

/** One element between the wheel event's target and the screen it happened on. */
export interface PaneScroller {
    readonly overflowX: string;
    readonly scrollWidth: number;
    readonly clientWidth: number;
    readonly scrollLeft: number;
}

/** Whether the session has another screen on either side of the one a gesture is on. */
export interface PanEnds {
    readonly hasPrevious: boolean;
    readonly hasNext: boolean;
}

/** Where a pull has put the track, in screens along it. */
export interface Panned {
    /** The screen the pull has moved onto, which the rest of it counts from. */
    readonly slot: number;
    /** What is left of the pull once that screen has been taken off it. */
    readonly raw: number;
    /** How far past that screen the finger is, after the ends of the session resist it. */
    readonly offset: number;
}

const scrollsSideways = (node: PaneScroller) => (node.overflowX === "auto" || node.overflowX === "scroll") && node.scrollWidth > node.clientWidth;

function atEdge(node: PaneScroller, deltaX: number): boolean {
    if (deltaX > 0) return node.scrollLeft >= node.scrollWidth - node.clientWidth - 1;
    return node.scrollLeft <= 0;
}

/**
 * Whether the stage takes this gesture. Decided on a gesture's first event and
 * kept until it ends: anything under the pointer that can still scroll sideways
 * keeps the whole gesture, so a pane that reaches its edge halfway through does
 * not hand the rest of the swipe to the stage.
 */
export function claimsWheel(chain: readonly PaneScroller[], deltaX: number, deltaY: number): boolean {
    if (chain.some((node) => scrollsSideways(node) && !atEdge(node, deltaX))) return false;
    return Math.abs(deltaX) > HORIZONTAL_RATIO * Math.abs(deltaY);
}

/**
 * Gives a little and then less and less, so the ends of a session feel like ends
 * rather than like something broken. Spread over about a screen of pull: give it
 * all away in the first fraction and the rest of the pull moves nothing at all.
 */
const resisted = (past: number) => (OVERSCROLL * GIVE * past) / (GIVE * past + OVERSCROLL);

/**
 * Where the track sits after a gesture has dragged `raw` screens, in screens
 * from the one it is on. The finger is followed one for one, except past the
 * first or last screen, where there is nothing to uncover.
 */
export function dragOffset(raw: number, ends: PanEnds): number {
    if (raw > 0 && !ends.hasNext) return resisted(raw);
    if (raw < 0 && !ends.hasPrevious) return -resisted(-raw);
    return raw;
}

/**
 * Where a pull of `raw` screens from `slot` puts the track. A screen pulled more
 * than halfway on is the screen the track is now counted from, and a screen
 * comes off the pull so the finger carries straight on into the next one. Done
 * over and over, so one long pull runs through as many screens as it reaches.
 */
export function panned(raw: number, slot: number, screens: number): Panned {
    for (;;) {
        const ends = { hasPrevious: slot > 0, hasNext: slot < screens - 1 };
        const offset = dragOffset(raw, ends);
        const step = offset > HANDOVER && ends.hasNext ? 1 : offset < -HANDOVER && ends.hasPrevious ? -1 : 0;
        if (step === 0) return { slot, raw, offset };
        slot += step;
        raw -= step;
    }
}
