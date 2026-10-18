import { useEffect, useRef } from "react";
import type { RefObject } from "react";
import * as cmd from "../state/commands";
import { selectSwipeOrder } from "../state/selectors";
import { getState } from "../state/store";
import { claimsWheel, dragOffset, landing, scrollersUnder, springHome, watchSwipes } from "../lib/swipe";
import type { SwipeHandle } from "../lib/swipe";
import type { WindowPan } from "./useWindowPan";

/** The gap the cards keep between them, which only the stylesheet knows. */
function cardGap(area: HTMLElement): number {
    // jsdom reports no custom properties, so a test stage has no gap.
    return Number.parseFloat(getComputedStyle(area).getPropertyValue("--window-card-gap")) || 0;
}

/** The live session's screens and the one it is on, read from the store rather than a render. */
function session() {
    const state = getState();
    return {
        order: selectSwipeOrder(state, state.activeSessionId),
        on: state.sessions[state.activeSessionId]?.activeWindowId ?? null,
    };
}

/**
 * Pages the session's screens with a two-finger trackpad swipe, one screen per
 * swipe. The track follows the hand, and when the hand lifts a spring carries it
 * from the hand's speed onto the screen it chose; the session only moves once the
 * track has arrived, so nothing re-renders while anything is moving.
 */
export function useWheelPan(areaRef: RefObject<HTMLElement | null>, pan: WindowPan): void {
    const latest = useRef(pan);
    latest.current = pan;

    useEffect(() => {
        const area = areaRef.current;
        if (!area) return;
        let finishLanding: (() => void) | null = null;

        const start = (event: WheelEvent): SwipeHandle | null => {
            // A stack's strip scrolls itself, and pages inside a pane swipe themselves.
            if (event.target instanceof Element && event.target.closest(".tabbar, .swipe-pages")) return null;
            finishLanding?.();
            const { order, on } = session();
            const stride = area.clientWidth + cardGap(area);
            const slot = on === null ? -1 : order.indexOf(on);
            if (on === null || stride <= 0 || slot < 0) return null;
            if (!claimsWheel(scrollersUnder(event.target, ".window-layer"), event.deltaX, event.deltaY)) return null;

            const ends = { hasPrevious: slot > 0, hasNext: slot < order.length - 1 };
            let raw = 0;
            let offset = 0;
            let toward: string | null = null;
            let held = false;
            let gone = false;
            let frame: number | null = null;

            // A switch from somewhere else, or a screen closing, moves the session
            // out from under the swipe, and the track goes back to whatever it is on now.
            const stillOn = () => {
                if (gone) return false;
                const now = session();
                if (now.on === on && now.order[slot] === on) return true;
                gone = true;
                if (held) latest.current.park();
                return false;
            };
            const paint = () => {
                frame = null;
                if (stillOn()) latest.current.place(slot + offset);
            };

            return {
                move(pixels) {
                    if (!stillOn()) return;
                    raw += pixels / stride;
                    offset = dragOffset(raw, ends);
                    const next = offset > 0 ? (order[slot + 1] ?? null) : offset < 0 ? (order[slot - 1] ?? null) : toward;
                    if (!held || next !== toward) {
                        held = true;
                        toward = next;
                        latest.current.grab(on, toward);
                    }
                    frame ??= requestAnimationFrame(paint);
                },
                release(speed) {
                    if (frame != null) cancelAnimationFrame(frame);
                    frame = null;
                    if (!held || !stillOn()) return;
                    const step = landing(offset, speed, ends);
                    const onto = order[slot + step] ?? on;
                    finishLanding = springHome(
                        offset - step,
                        speed / stride,
                        (at) => {
                            if (stillOn()) latest.current.place(slot + step + at);
                        },
                        () => {
                            finishLanding = null;
                            if (!stillOn()) return;
                            if (onto === on) return latest.current.park();
                            latest.current.land(onto);
                            cmd.selectWindowId(onto);
                        },
                    );
                },
            };
        };

        const stop = watchSwipes(area, start);
        return () => {
            stop();
            finishLanding?.();
        };
    }, [areaRef]);
}
