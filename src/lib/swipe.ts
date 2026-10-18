import { prefersReducedMotion } from "./motion";
import { fingersDown, onFingers } from "./wheelTouch";

/**
 * A two-finger trackpad swipe that pages between things laid side by side, the
 * way Zen pages between spaces: the page follows the hand one for one, moves at
 * most one page per swipe, and when the hand lifts a spring carries it from the
 * hand's own speed onto the page it was going for.
 */

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
/** Where nothing watches the trackpad the two look alike, so one wait has to suit both. */
export const UNWATCHED_END_MS = 600;
const endDelay = (down: boolean | null) => (down === null ? UNWATCHED_END_MS : down ? HELD_END_MS : SPENT_END_MS);

/** Below this the gesture is diagonal enough to belong to whatever is under it. */
const HORIZONTAL_RATIO = 1.5;
/** How far a page can be pulled past the last one there is. */
const OVERSCROLL = 0.15;
/** How much of the pull gets through before the resistance takes over. */
const GIVE = 0.55;

/** How far onto the next page a hand has to set a swipe down for it to take that page. */
const COMMIT = 0.25;
/**
 * How fast a hand still moving as it lifts has to be going for the swipe to take
 * the next page however little of it is showing, in pixels of hand a second. In
 * pixels rather than pages, because a hand does not move further because the
 * screen is wider.
 */
const FLICK_PX_S = 500;
/** A hand lifting while moving back this fast has changed its mind. */
const RECOIL_PX_S = 200;
/** Only the last moments before the hand lifts say how fast it was going. */
const SPEED_WINDOW_MS = 80;

/** How quickly the landing closes: the spring settles in about a quarter of a second, without bouncing. */
const SPRING_RATE = 25;
const SPRING_REST = 0.0005;
const SPRING_LONGEST_MS = 600;

/** One element between the wheel event's target and the pages it happened on. */
export interface PaneScroller {
    readonly overflowX: string;
    readonly scrollWidth: number;
    readonly clientWidth: number;
    readonly scrollLeft: number;
}

/** Whether there is another page on either side of the one a swipe starts on. */
export interface PageEnds {
    readonly hasPrevious: boolean;
    readonly hasNext: boolean;
}

const scrollsSideways = (node: PaneScroller) => (node.overflowX === "auto" || node.overflowX === "scroll") && node.scrollWidth > node.clientWidth;

function atEdge(node: PaneScroller, deltaX: number): boolean {
    if (deltaX > 0) return node.scrollLeft >= node.scrollWidth - node.clientWidth - 1;
    return node.scrollLeft <= 0;
}

/**
 * Whether the pages take this gesture. Decided on a gesture's first event and
 * kept until it ends: anything under the pointer that can still scroll sideways
 * keeps the whole gesture, so a pane that reaches its edge halfway through does
 * not hand the rest of the swipe to the pages.
 */
export function claimsWheel(chain: readonly PaneScroller[], deltaX: number, deltaY: number): boolean {
    if (chain.some((node) => scrollsSideways(node) && !atEdge(node, deltaX))) return false;
    return Math.abs(deltaX) > HORIZONTAL_RATIO * Math.abs(deltaY);
}

/** Everything between `target` and the page it is on that might want to scroll sideways instead. */
export function scrollersUnder(target: EventTarget | null, page: string): PaneScroller[] {
    const chain: PaneScroller[] = [];
    let node = target instanceof Element ? target : null;
    while (node && !node.matches(page)) {
        const style = getComputedStyle(node);
        chain.push({ overflowX: style.overflowX, scrollWidth: node.scrollWidth, clientWidth: node.clientWidth, scrollLeft: node.scrollLeft });
        node = node.parentElement;
    }
    return chain;
}

/**
 * Gives a little and then less and less, so an end feels like an end rather
 * than like something broken. Spread over about a page of pull: give it all
 * away in the first fraction and the rest of the pull moves nothing at all.
 */
const resisted = (past: number) => (OVERSCROLL * GIVE * past) / (GIVE * past + OVERSCROLL);

/**
 * Where the pages sit after a swipe has dragged `raw` pages, in pages from the
 * one it started on. The hand is followed one for one up to the next page, and
 * resisted past it or past an end, so one swipe never uncovers more than one page.
 */
export function dragOffset(raw: number, ends: PageEnds): number {
    const way = Math.sign(raw);
    const reach = (way > 0 ? ends.hasNext : ends.hasPrevious) ? 1 : 0;
    const past = Math.abs(raw) - reach;
    return past <= 0 ? raw : way * (reach + resisted(past));
}

/**
 * Which page a swipe lands on, -1, 0 or 1 from the one it started on, out of
 * how far it got and how fast the hand was going as it lifted, in pixels a
 * second along the same axis as the offset.
 */
export function landing(offset: number, speed: number, ends: PageEnds): number {
    const way = Math.sign(offset);
    if (way === 0 || !(way > 0 ? ends.hasNext : ends.hasPrevious)) return 0;
    const along = speed * way;
    if (along <= -RECOIL_PX_S) return 0;
    return Math.abs(offset) >= COMMIT || along >= FLICK_PX_S ? way : 0;
}

/** One wheel event's push, and when it came. */
interface Push {
    readonly at: number;
    readonly pixels: number;
}

/** How fast the hand was going at `until`, in pixels a second, from the pushes just before it. */
export function speedOf(pushes: readonly Push[], until: number): number {
    const recent = pushes.filter((push) => until - push.at <= SPEED_WINDOW_MS);
    if (recent.length < 2) return 0;
    const span = recent[recent.length - 1].at - recent[0].at;
    if (span <= 0) return 0;
    let moved = 0;
    for (const push of recent.slice(1)) moved += push.pixels;
    return (moved / span) * 1000;
}

/**
 * Carries a value from `from` to rest at nought, starting at `velocity` units a
 * second, on a critically damped spring: it picks up at the hand's speed and
 * never overshoots, so it cannot uncover a page past the one it lands on.
 * Returns a function that jumps to the end at once.
 */
export function springHome(from: number, velocity: number, frame: (at: number) => void, done: () => void): () => void {
    if (prefersReducedMotion() || from === 0) {
        frame(0);
        done();
        return () => {};
    }
    // Faster than this towards home and the spring would cross it before settling.
    const limit = SPRING_RATE * Math.abs(from);
    const v = Math.max(-limit, Math.min(limit, velocity));
    const lean = v + SPRING_RATE * from;
    const start = performance.now();
    let handle: number | null = null;
    const finish = () => {
        if (handle != null) cancelAnimationFrame(handle);
        handle = null;
        frame(0);
        done();
    };
    const step = (now: number) => {
        const t = Math.max(0, now - start) / 1000;
        const decay = Math.exp(-SPRING_RATE * t);
        const at = (from + lean * t) * decay;
        const moving = (lean - SPRING_RATE * (from + lean * t)) * decay;
        if ((Math.abs(at) < SPRING_REST && Math.abs(moving) < SPRING_REST * SPRING_RATE) || now - start >= SPRING_LONGEST_MS) return finish();
        frame(at);
        handle = requestAnimationFrame(step);
    };
    handle = requestAnimationFrame(step);
    return () => {
        if (handle != null) finish();
    };
}

/** What a claimed swipe does with the hand: follows it, then lands when it lifts. */
export interface SwipeHandle {
    move(pixels: number): void;
    /** The hand has gone, still moving at `speed` pixels a second. */
    release(speed: number): void;
}

interface Gesture {
    readonly handle: SwipeHandle | null;
    pushes: Push[];
    /** Whether the swipe has landed, so what still arrives is only its glide. */
    spent: boolean;
}

/**
 * Turns the wheel events over `area` into swipes. `start` sees each swipe's
 * first event and either takes the swipe or leaves it to whatever is under it.
 */
export function watchSwipes(area: HTMLElement, start: (event: WheelEvent) => SwipeHandle | null): () => void {
    let gesture: Gesture | null = null;
    let quiet: number | null = null;

    const land = () => {
        const done = gesture;
        if (!done || done.spent) return;
        done.spent = true;
        done.handle?.release(speedOf(done.pushes, performance.now()));
    };

    const forget = () => {
        if (quiet != null) window.clearTimeout(quiet);
        quiet = null;
        gesture = null;
    };

    const onWheel = (event: WheelEvent) => {
        gesture ??= { handle: start(event), pushes: [], spent: false };
        const moving = gesture;
        if (quiet != null) window.clearTimeout(quiet);
        quiet = window.setTimeout(() => {
            land();
            forget();
        }, endDelay(fingersDown()));
        if (!moving.handle) return;
        // Whatever is underneath must not scroll as well, including a terminal
        // that turns wheel gestures into cursor keys.
        event.preventDefault();
        if (moving.spent) return;
        const at = performance.now();
        moving.pushes = moving.pushes.filter((push) => at - push.at <= SPEED_WINDOW_MS);
        moving.pushes.push({ at, pixels: event.deltaX });
        moving.handle.move(event.deltaX);
    };

    const watched = onFingers((down) => {
        // The hand leaving is the end of the swipe, not the glide it leaves after it.
        if (!down) return land();
        if (gesture?.spent) forget();
    });
    area.addEventListener("wheel", onWheel, { passive: false, capture: true });
    return () => {
        area.removeEventListener("wheel", onWheel, { capture: true });
        watched();
        forget();
    };
}
