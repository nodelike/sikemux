import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { RefObject } from "react";
import { prefersReducedMotion } from "../lib/motion";
import { performanceTelemetry } from "../lib/performance";

/** How long the track takes to travel one screen. */
export const PAN_MS = 280;
/** Any shorter and a settle reads as a cut rather than a move. */
const SETTLE_MIN_MS = 120;
/** Falls back to this when no `transitionend` arrives, so a pan can never get stuck. */
const SETTLE_GUARD_MS = PAN_MS + 120;

/**
 * How long a swipe put back where it came from takes. One time for all of them,
 * however far the pull got: a band pulled twice as far comes back twice as fast
 * rather than taking twice as long, and it is the time that a person reads as
 * the feel of the thing.
 */
export const RETURN_MS = 200;

/**
 * How long to take over the ground a gesture left, which is less than a screen.
 * Not in proportion to it: covering half the distance in half the time looks
 * like the same speed, so a short trip keeps some of the pace of a long one.
 */
export const settleMs = (remaining: number) => Math.max(SETTLE_MIN_MS, Math.round(PAN_MS * Math.sqrt(Math.min(Math.max(remaining, 0), 1))));

/** How far the track is slid left to bring `index` to the front. One screen of
 *  travel is the stage plus the gap the cards keep between them. */
export const panOffset = (index: number) => `calc(${-index} * (100% + var(--window-card-gap)))`;

interface Pan {
    /** A drag is written to the element frame by frame; a slide travels on a transition. */
    readonly kind: "drag" | "slide";
    readonly from: string;
    /** The screen beside it, or none while a drag pulls against the end of the session. */
    readonly to: string | null;
    /** Where the window being left sits for the whole slide, in screen widths from the track's left edge. */
    readonly fromSlot: number;
    /** Where the target is parked for the slide, so the travel is one screen however far the jump was. */
    readonly slot: number;
    readonly distance: number;
    /** How long the travel takes. A gesture hands back less than a screen of it. */
    readonly ms: number;
    /** Whether the travel runs back against the hand rather than on the way it went. */
    readonly returning: boolean;
}

export interface WindowPan {
    readonly trackRef: RefObject<HTMLDivElement | null>;
    readonly panning: boolean;
    /** Whether the track is past its parked position and actually travelling. */
    readonly sliding: boolean;
    /** Whether what is travelling is a swipe coming back rather than carrying on. */
    readonly returning: boolean;
    /** Where the track sits now, in screen widths from its left edge. */
    readonly at: number;
    /** How long the travel on screen now takes, which the stylesheet needs. */
    readonly ms: number;
    /** Where a layer sits now, which is its own slot unless a slide has it parked somewhere else. */
    slotOf(windowId: string, slot: number): number;
    paints(windowId: string): boolean;
    /**
     * Hands the track to a gesture: `on` and `beside` paint, `--pan` is the
     * gesture's to write, and a session moving onto `on` is the gesture's own
     * doing rather than a switch to slide for.
     */
    grab(on: string, beside: string | null): void;
    /**
     * Takes the track back when the gesture stops, sliding from wherever it left
     * it onto `onto` over `ms`. The gesture knows how much ground that leaves.
     */
    snap(onto: string, beside: string | null, ms: number, returning: boolean): void;
    /**
     * Takes the track back with no travel at all, parked on the screen the
     * session is on now. For a gesture whose screens moved out from under it,
     * which has nowhere left to slide from.
     */
    park(): void;
}

function planPan(from: string | null, to: string | null, slots: ReadonlyMap<string, number>, running: Pan | null): Pan | null {
    if (!from || !to || from === to || prefersReducedMotion()) return null;
    const home = slots.get(from);
    const toSlot = slots.get(to);
    // A window that has left the session has nothing to slide out, so the switch cuts.
    if (home === undefined || toSlot === undefined) return null;
    // A switch made mid-slide leaves the window the slide was bringing in, which
    // is sitting where that slide parked it rather than on its own screen.
    const fromSlot = running?.to === from ? running.slot : home;
    // Taking over a slide already travelling is that same travel carrying on, so
    // it carries on at the same pace rather than starting a fresh screen's worth.
    const ms = running?.to === from ? running.ms : PAN_MS;
    return { kind: "slide", from, to, fromSlot, slot: fromSlot + (toSlot > home ? 1 : -1), distance: Math.abs(toSlot - home), ms, returning: false };
}

/**
 * Slides the track one screen whenever the session moves to another window.
 *
 * The store has already committed the new active window by the time this runs,
 * so the strip and the keyboard are correct while the slide is still going. A
 * jump of several screens parks its target next to the one being left and puts
 * it back on settle, which keeps the travel and the number of painted layers
 * the same whether the jump was one screen or twenty.
 *
 * A trackpad gesture borrows the track through `grab` and `snap` instead of a
 * switch. It drives `--pan` itself and moves the session along the screens it
 * drags past, so only the snap at the end is a slide.
 */
export function useWindowPan(sessionId: string, activeWindowId: string | null, slots: ReadonlyMap<string, number>): WindowPan {
    const trackRef = useRef<HTMLDivElement>(null);
    const [pan, setPan] = useState<Pan | null>(null);
    const [running, setRunning] = useState(false);
    const previous = useRef({ sessionId, activeWindowId });

    if (previous.current.sessionId !== sessionId || previous.current.activeWindowId !== activeWindowId) {
        const was = previous.current;
        previous.current = { sessionId, activeWindowId };
        // Another session is another track, so its switch is not a slide along this one.
        const next = was.sessionId === sessionId ? planPan(was.activeWindowId, activeWindowId, slots, pan) : null;
        setPan(next);
        // A slide chaining onto the one already travelling starts from where the
        // track is, so only a fresh one has to park first.
        setRunning(running && next !== null && pan?.to === next.from);
    }

    useLayoutEffect(() => {
        if (!pan || pan.kind === "drag") return;
        if (running) {
            // A slide taking over from a gesture starts where the gesture left the
            // track, which React had no reason to write, so put the destination on
            // the element itself now that the transition is on.
            trackRef.current?.style.setProperty("--pan", panOffset(pan.slot));
            return;
        }
        // Reading layout pins the parked position as the value the slide starts from.
        trackRef.current?.getBoundingClientRect();
        setRunning(true);
    }, [pan, running]);

    useEffect(() => {
        if (!pan || !running) return;
        const track = trackRef.current;
        const span = performanceTelemetry.startTrace("tab-pan", { distance: pan.distance, teleported: pan.distance > 1 });
        const settle = () => setPan(null);
        const guard = window.setTimeout(settle, SETTLE_GUARD_MS);
        const onEnd = (event: TransitionEvent) => {
            if (event.target === track && event.propertyName === "transform") settle();
        };
        track?.addEventListener("transitionend", onEnd);
        return () => {
            window.clearTimeout(guard);
            track?.removeEventListener("transitionend", onEnd);
            const recorded = performanceTelemetry.endSpan(span);
            if (recorded) performanceTelemetry.recordLatency("tab-pan", recorded.durationMs);
        };
    }, [pan, running]);

    const grab = (on: string, beside: string | null) => {
        const home = slots.get(on);
        if (home === undefined) return;
        // The gesture walks the session along the screens it drags past, so the
        // check above has to know those moves are already accounted for.
        previous.current = { sessionId, activeWindowId: on };
        const neighbour = beside === null ? undefined : slots.get(beside);
        // `fromSlot` is what React writes to `--pan`, and the gesture is writing
        // that itself, so it may not move while the gesture holds the track.
        setPan((was) => ({
            kind: "drag",
            from: on,
            to: neighbour === undefined ? null : beside,
            fromSlot: was?.kind === "drag" ? was.fromSlot : home,
            slot: home,
            distance: 1,
            ms: PAN_MS,
            returning: false,
        }));
        setRunning(false);
    };

    const park = () => {
        // A switch made mid-gesture plans a slide of its own, and that slide is
        // then the one thing moving the track: only a drag is still the gesture's.
        if (pan?.kind !== "drag") return;
        previous.current = { sessionId, activeWindowId };
        // The track goes by hand: React's `--pan` has not moved since the
        // gesture took the track over.
        trackRef.current?.style.setProperty("--pan", panOffset(activeWindowId === null ? 0 : (slots.get(activeWindowId) ?? 0)));
        setPan(null);
        setRunning(false);
    };

    const snap = (onto: string, beside: string | null, ms: number, returning: boolean) => {
        const home = slots.get(onto);
        // A screen closed under the swipe has nothing left to land on.
        if (home === undefined) return park();
        // A swipe thrown at the last moment moves the session one more screen on
        // its way out, and this is the travel for it, so nothing else plans one.
        previous.current = { sessionId, activeWindowId: onto };
        if (prefersReducedMotion()) {
            trackRef.current?.style.setProperty("--pan", panOffset(home));
            setPan(null);
            setRunning(false);
            return;
        }
        // The same slide a switch makes, settling the same way, except that the
        // two screens are already side by side so neither has to be parked.
        const neighbour = beside === null ? undefined : slots.get(beside);
        setPan({ kind: "slide", from: beside ?? onto, to: onto, fromSlot: neighbour ?? home, slot: home, distance: 1, ms, returning });
        setRunning(true);
    };

    return {
        trackRef,
        panning: pan !== null,
        sliding: pan !== null && pan.kind === "slide" && running,
        returning: pan?.returning === true,
        ms: pan?.ms ?? PAN_MS,
        at: pan ? (running ? pan.slot : pan.fromSlot) : activeWindowId ? (slots.get(activeWindowId) ?? 0) : 0,
        slotOf: (windowId, slot) => {
            // A drag moves the whole track rather than parking anything, so every
            // screen stays on its own.
            if (!pan || pan.kind === "drag") return slot;
            if (windowId === pan.to) return pan.slot;
            return windowId === pan.from ? pan.fromSlot : slot;
        },
        paints: (windowId) => (pan ? windowId === pan.from || windowId === pan.to : windowId === activeWindowId),
        grab,
        snap,
        park,
    };
}
