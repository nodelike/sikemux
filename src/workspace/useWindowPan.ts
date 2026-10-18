import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { RefObject } from "react";
import { prefersReducedMotion } from "../lib/motion";
import { performanceTelemetry } from "../lib/performance";

/** How long the track takes to travel one screen. */
export const PAN_MS = 280;
/** Falls back to this when no `transitionend` arrives, so a pan or a slide can never get stuck. */
export const SETTLE_GUARD_MS = PAN_MS + 120;

/** How far the track is slid left to bring `index` to the front. One screen of
 *  travel is the stage plus the gap the cards keep between them. */
export const panOffset = (index: number) => `calc(${-index} * (100% + var(--window-card-gap)))`;

/** The track's transform for `index`. Written as the transform itself rather than
 *  a custom property, which every screen on the track would inherit and restyle for. */
export const panTransform = (index: number) => `translate(${panOffset(index)}, 0)`;

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
    /** How long the travel takes. */
    readonly ms: number;
}

export interface WindowPan {
    readonly trackRef: RefObject<HTMLDivElement | null>;
    readonly panning: boolean;
    /** Whether the track is past its parked position and actually travelling. */
    readonly sliding: boolean;
    /** Where the track sits now, in screen widths from its left edge. */
    readonly at: number;
    /** How long the travel on screen now takes, which the stylesheet needs. */
    readonly ms: number;
    /** Where a layer sits now, which is its own slot unless a slide has it parked somewhere else. */
    slotOf(windowId: string, slot: number): number;
    paints(windowId: string): boolean;
    /**
     * Hands the track to a gesture: `on` and `beside` paint, the track's transform
     * is the gesture's to write, and a session moving onto `on` is the gesture's own
     * doing rather than a switch to slide for.
     */
    grab(on: string, beside: string | null): void;
    /** Puts the track `at` screens along while a gesture holds it. */
    place(at: number): void;
    /**
     * Takes the track back from a gesture that has carried it all the way onto
     * `onto`, which the session is about to move to without a slide of its own.
     */
    land(onto: string): void;
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
    return { kind: "slide", from, to, fromSlot, slot: fromSlot + (toSlot > home ? 1 : -1), distance: Math.abs(toSlot - home), ms };
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
 * A trackpad gesture borrows the track through `grab`, `place` and `land`
 * instead of a switch. It drives the transform itself, landing included, and
 * the session only moves once the track is already there.
 */
export function useWindowPan(sessionId: string, activeWindowId: string | null, slots: ReadonlyMap<string, number>): WindowPan {
    const trackRef = useRef<HTMLDivElement>(null);
    const [pan, setPan] = useState<Pan | null>(null);
    const [running, setRunning] = useState(false);
    const previous = useRef({ sessionId, activeWindowId });
    const held = useRef<number | null>(null);

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
            if (trackRef.current) trackRef.current.style.transform = panTransform(pan.slot);
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
        // `fromSlot` is what React writes to the transform, and the gesture is writing
        // that itself, so it may not move while the gesture holds the track.
        setPan((was) => ({
            kind: "drag",
            from: on,
            to: neighbour === undefined ? null : beside,
            fromSlot: was?.kind === "drag" ? was.fromSlot : home,
            slot: home,
            distance: 1,
            ms: PAN_MS,
        }));
        setRunning(false);
    };

    const place = (at: number) => {
        held.current = at;
        if (trackRef.current) trackRef.current.style.transform = panTransform(at);
    };

    const giveBack = (onto: string | null) => {
        held.current = null;
        previous.current = { sessionId, activeWindowId: onto };
        if (trackRef.current) trackRef.current.style.transform = panTransform(onto === null ? 0 : (slots.get(onto) ?? 0));
        setPan(null);
        setRunning(false);
    };

    // A switch made mid-gesture plans a slide of its own, and that slide is
    // then the one thing moving the track: only a drag is still the gesture's.
    const park = () => {
        if (pan?.kind === "drag") giveBack(activeWindowId);
    };

    const land = (onto: string) => {
        if (pan?.kind !== "drag") return;
        if (!slots.has(onto)) return giveBack(activeWindowId);
        giveBack(onto);
    };

    return {
        trackRef,
        panning: pan !== null,
        sliding: pan !== null && pan.kind === "slide" && running,
        ms: pan?.ms ?? PAN_MS,
        at: pan
            ? pan.kind === "drag"
                ? (held.current ?? pan.fromSlot)
                : running
                  ? pan.slot
                  : pan.fromSlot
            : activeWindowId
              ? (slots.get(activeWindowId) ?? 0)
              : 0,
        slotOf: (windowId, slot) => {
            // A drag moves the whole track rather than parking anything, so every
            // screen stays on its own.
            if (!pan || pan.kind === "drag") return slot;
            if (windowId === pan.to) return pan.slot;
            return windowId === pan.from ? pan.fromSlot : slot;
        },
        paints: (windowId) => (pan ? windowId === pan.from || windowId === pan.to : windowId === activeWindowId),
        grab,
        place,
        land,
        park,
    };
}
