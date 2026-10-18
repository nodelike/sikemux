import { type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { claimsWheel, dragOffset, landing, scrollersUnder, springHome, watchSwipes } from "../lib/swipe";
import type { SwipeHandle } from "../lib/swipe";
import { Tooltip } from "./Tooltip";

export interface SwipePage {
    readonly id: string;
    readonly label: string;
    readonly icon: ReactNode;
    readonly content: ReactNode;
}

const pageTransform = (at: number) => `translate3d(${-at * 100}%, 0, 0)`;

/**
 * Pages side by side, chosen from the icons at their foot or swiped between with
 * two fingers, one page per swipe. The track is moved by hand, never by a
 * render, so a swipe costs no React work until it has landed.
 */
export function SwipePages({
    pages,
    current,
    onChange,
    label,
    className,
}: {
    pages: readonly SwipePage[];
    current: string;
    onChange: (id: string) => void;
    label: string;
    className?: string;
}) {
    const index = Math.max(
        0,
        pages.findIndex((page) => page.id === current),
    );
    const [firstIndex] = useState(index);
    const rootRef = useRef<HTMLDivElement>(null);
    const viewRef = useRef<HTMLDivElement>(null);
    const trackRef = useRef<HTMLDivElement>(null);
    /** Where the track is, where it is going, and how to finish getting there. */
    const motion = useRef({ at: index, target: index, finish: null as (() => void) | null });
    const latest = useRef({ ids: pages.map((page) => page.id), onChange });
    latest.current = { ids: pages.map((page) => page.id), onChange };

    const place = (at: number) => {
        motion.current.at = at;
        if (trackRef.current) trackRef.current.style.transform = pageTransform(at);
    };

    const glide = (target: number, velocity: number) => {
        const m = motion.current;
        const view = viewRef.current;
        m.finish?.();
        m.target = target;
        if (view) view.dataset.moving = "";
        m.finish = springHome(
            m.at - target,
            velocity,
            (at) => place(target + at),
            () => {
                m.finish = null;
                if (view) delete view.dataset.moving;
            },
        );
    };
    const glideRef = useRef(glide);
    glideRef.current = glide;

    useLayoutEffect(() => {
        if (motion.current.target !== index) glideRef.current(index, 0);
    }, [index]);

    useEffect(() => {
        const root = rootRef.current;
        const view = viewRef.current;
        if (!root || !view) return;
        const start = (event: WheelEvent): SwipeHandle | null => {
            if (!claimsWheel(scrollersUnder(event.target, ".swipe-page, .swipe-switch"), event.deltaX, event.deltaY)) return null;
            const width = view.clientWidth;
            if (width <= 0) return null;
            const m = motion.current;
            m.finish?.();
            const base = m.target;
            const ends = { hasPrevious: base > 0, hasNext: base < latest.current.ids.length - 1 };
            let raw = 0;
            let offset = 0;
            let frame: number | null = null;
            view.dataset.moving = "";
            return {
                move(pixels) {
                    raw += pixels / width;
                    offset = dragOffset(raw, ends);
                    frame ??= requestAnimationFrame(() => {
                        frame = null;
                        if (trackRef.current) trackRef.current.style.transform = pageTransform(base + offset);
                        m.at = base + offset;
                    });
                },
                release(speed) {
                    if (frame != null) cancelAnimationFrame(frame);
                    m.at = base + offset;
                    const target = base + landing(offset, speed, ends);
                    glideRef.current(target, speed / width);
                    if (target !== base) latest.current.onChange(latest.current.ids[target]);
                },
            };
        };
        return watchSwipes(root, start);
    }, []);

    return (
        <div className={`swipe-pages${className ? ` ${className}` : ""}`} ref={rootRef}>
            <div className="swipe-pager" ref={viewRef}>
                <div className="swipe-track" ref={trackRef} style={{ transform: pageTransform(firstIndex) }}>
                    {pages.map((page, at) => (
                        <section key={page.id} className={`swipe-page${at === index ? " is-current" : ""}`} aria-label={page.label}>
                            {page.content}
                        </section>
                    ))}
                </div>
            </div>
            <div className="swipe-switch" role="tablist" aria-label={label}>
                {pages.map((page) => (
                    <Tooltip key={page.id} label={page.label}>
                        <button
                            type="button"
                            role="tab"
                            aria-selected={page.id === current}
                            aria-label={page.label}
                            onClick={() => onChange(page.id)}>
                            {page.icon}
                        </button>
                    </Tooltip>
                ))}
            </div>
        </div>
    );
}
