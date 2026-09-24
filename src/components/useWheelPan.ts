import { useEffect, useRef } from "react";
import type { RefObject } from "react";
import * as cmd from "../state/commands";
import { fingersDown, onFingers, watchFingers } from "../lib/wheelTouch";
import { selectSwipeOrder } from "../state/selectors";
import { getState } from "../state/store";
import { panOffset, RETURN_MS, settleMs } from "./useWindowPan";
import type { WindowPan } from "./useWindowPan";
import { claimsWheel, endDelay, flicked, panned, pulledOn, pushed, thrust } from "./wheelPan";
import type { PaneScroller, Push } from "./wheelPan";

interface Gesture {
    /** Whether the stage took this gesture, decided once on its first event. */
    readonly claimed: boolean;
    /** One screen of finger travel in pixels: the stage plus the gap between cards. */
    readonly stride: number;
    /** The screen the track is based on, which a crossing moves along. */
    slot: number;
    /** Screens dragged from that screen, before the ends of the session resist the pull. */
    raw: number;
    /** How far past that screen the track sits, which is what reaches `--pan`. */
    offset: number;
    /** The screen showing beside it, which is the one the drag is heading for. */
    toward: string | null;
    /** The way the hand was last going, which is not the way the track sits once a pull has crossed. */
    way: number;
    /** Whether React has been handed the pair of screens the gesture is between. */
    held: boolean;
    /** The last moments of the swipe, which say whether it was thrown or placed. */
    pushes: readonly Push[];
    /** Whether the swipe has already landed, so what still arrives is only its tail. */
    spent: boolean;
    frame: number | null;
}

/** The gap the cards keep between them, which only the stylesheet knows. */
function cardGap(area: HTMLElement): number {
    // jsdom reports no custom properties, so a test stage has no gap.
    return Number.parseFloat(getComputedStyle(area).getPropertyValue("--window-card-gap")) || 0;
}

/** Everything between the wheel event and its screen that might want to scroll sideways instead. */
function scrollersUnder(target: EventTarget | null): PaneScroller[] {
    const chain: PaneScroller[] = [];
    let node = target instanceof Element ? target : null;
    while (node && !node.classList.contains("window-layer")) {
        const style = getComputedStyle(node);
        chain.push({ overflowX: style.overflowX, scrollWidth: node.scrollWidth, clientWidth: node.clientWidth, scrollLeft: node.scrollLeft });
        node = node.parentElement;
    }
    return chain;
}

/**
 * Drags the track sideways with a two-finger trackpad swipe, the way a paged
 * scroller does: the track follows the finger for as long as the finger moves,
 * and a screen dragged more than halfway on is the screen the session is on.
 *
 * Making it active is not a landing. The track keeps following the finger from
 * the new screen, so a long swipe runs through as many screens as it has reach
 * while only the two either side of the finger ever paint. Only when the events
 * stop does anything animate, and then only to close the last half screen.
 *
 * Nothing here may wait on React. `--pan` is written straight to the element
 * from a frame loop, the session is read back out of the store rather than off
 * a prop, and React is told two things: which screen is active and which layers
 * paint.
 */
export function useWheelPan(areaRef: RefObject<HTMLElement | null>, pan: WindowPan): void {
    const latest = useRef(pan);
    latest.current = pan;

    useEffect(() => {
        const area = areaRef.current;
        if (!area) return;
        let gesture: Gesture | null = null;
        let quiet: number | null = null;

        /** The live session's screens and the one it is on, read where the gesture
         *  put them rather than where a render would have them. */
        const session = () => {
            const state = getState();
            return {
                order: selectSwipeOrder(state, state.activeSessionId),
                on: state.sessions[state.activeSessionId]?.activeWindowId ?? null,
            };
        };

        const forget = () => {
            if (gesture?.frame != null) cancelAnimationFrame(gesture.frame);
            if (quiet != null) window.clearTimeout(quiet);
            gesture = null;
            quiet = null;
        };

        /** Drops a gesture that has nothing left to hold, handing the track back
         *  rather than leaving it parked where the finger was. */
        const letGo = () => {
            if (gesture?.held) latest.current.park();
            forget();
        };

        const paint = () => {
            const moving = gesture;
            if (!moving) return;
            moving.frame = null;
            const { order, on } = session();
            if (order[moving.slot] !== on) return;
            latest.current.trackRef.current?.style.setProperty("--pan", panOffset(moving.slot + moving.offset));
        };

        /**
         * Closes the swipe onto a screen. The finger leaves the track at most half
         * a screen from the one the session is on, unless the swipe was thrown
         * rather than placed, which carries it one screen further the way it went.
         *
         * The close either carries on the way the hand was going or turns round and
         * goes back against it, and those are two different movements: one picks up
         * where the hand left off, the other has to stop the track first. A fast
         * swipe put back at the same pace it was taken away at is the one that
         * reads as broken, so a swipe coming back always comes back the same way.
         */
        const land = (until: number) => {
            const done = gesture;
            if (!done || done.spent) return;
            done.spent = true;
            if (done.frame != null) cancelAnimationFrame(done.frame);
            done.frame = null;
            if (!done.claimed || !done.held) return;
            const { order, on } = session();
            if (on === null || order[done.slot] !== on) return latest.current.park();
            // Thrown at the next screen, or else pulled far enough onto it to have
            // chosen it. A throw only carries the swipe onto a screen it has already
            // uncovered: once a pull has crossed onto a screen it is counted from that
            // one, and the ground that carried it there is the same ground the throw
            // reads, so spending it twice jumps a screen and slides over one nobody
            // painted.
            const flick = flicked(done.pushes, until) || pulledOn(done.offset, done.way);
            const thrown = flick * done.offset < 0 ? 0 : flick;
            const onto = (thrown === 0 ? null : (order[done.slot + thrown] ?? null)) ?? on;
            const travel = (onto === on ? 0 : thrown) - done.offset;
            const returning = travel * thrust(done.pushes, until) < 0;
            if (onto !== on) {
                // The glide still to come has to find the swipe where it landed,
                // or it reads as a switch from elsewhere and starts a swipe of its own.
                done.slot += thrown;
                cmd.selectWindowId(onto);
            }
            latest.current.snap(onto, onto === on ? done.toward : on, returning ? RETURN_MS : settleMs(Math.abs(travel)), returning);
        };

        const settle = () => {
            land(performance.now());
            forget();
        };

        const onWheel = (event: WheelEvent) => {
            // The strip sits on the stage and scrolls itself.
            if (event.target instanceof Element && event.target.closest(".tabbar")) return;
            const { order, on } = session();
            // A switch from somewhere else takes the track away, and the pan the
            // gesture was driving is that switch's slide by now.
            if (gesture && order[gesture.slot] !== on) letGo();
            if (!gesture) {
                const stride = area.clientWidth + cardGap(area);
                const slot = on === null ? -1 : order.indexOf(on);
                if (stride <= 0 || slot < 0) return;
                gesture = {
                    claimed: claimsWheel(scrollersUnder(event.target), event.deltaX, event.deltaY),
                    stride,
                    slot,
                    raw: 0,
                    offset: 0,
                    toward: null,
                    way: 0,
                    held: false,
                    pushes: [],
                    spent: false,
                    frame: null,
                };
            }
            const moving = gesture;
            if (quiet != null) window.clearTimeout(quiet);
            quiet = window.setTimeout(settle, endDelay(fingersDown()));
            if (!moving.claimed) return;
            // Whatever is underneath must not scroll as well, including a terminal
            // that turns wheel gestures into cursor keys.
            event.preventDefault();
            // Everything arriving after the hand left is the tail of a swipe that
            // has already landed, not more of it.
            if (moving.spent) return;

            const at = performance.now();
            moving.pushes = pushed(moving.pushes, at, event.deltaX);
            moving.way = Math.sign(event.deltaX) || moving.way;
            const was = moving.slot;
            const now = panned(moving.raw + event.deltaX / moving.stride, moving.slot, order.length);
            moving.slot = now.slot;
            moving.raw = now.raw;
            moving.offset = now.offset;
            const toward =
                moving.offset > 0 ? (order[moving.slot + 1] ?? null) : moving.offset < 0 ? (order[moving.slot - 1] ?? null) : moving.toward;
            if (moving.slot !== was) cmd.selectWindowId(order[moving.slot]);
            if (!moving.held || moving.slot !== was || toward !== moving.toward) {
                moving.held = true;
                moving.toward = toward;
                latest.current.grab(order[moving.slot], toward);
            }
            if (moving.frame == null) moving.frame = requestAnimationFrame(paint);
        };

        const watched = onFingers((down) => {
            // The hand leaving is the end of the swipe. Waiting for the glide it
            // left to run out would hold the track still for as long as that took.
            if (!down) return land(performance.now());
            // And a hand coming back down is a new swipe, whatever the last one left.
            if (gesture?.spent) forget();
        });
        const watching = new AbortController();
        watchFingers(watching.signal);
        area.addEventListener("wheel", onWheel, { passive: false, capture: true });
        return () => {
            area.removeEventListener("wheel", onWheel, { capture: true });
            watching.abort();
            watched();
            forget();
        };
    }, [areaRef]);
}
