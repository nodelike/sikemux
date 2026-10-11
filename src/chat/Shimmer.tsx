import { useLayoutEffect, useRef, type ReactNode } from "react";

/** Text inside these is a painted copy, not something the reader sees twice. */
export const SHIMMER_COPY = ".chat-shimmer-band";

/* WebKit works out a running slide's distance once, so after the label
   resizes a band would keep moving its old width and cut the text short until
   the pass ends. Restarting each slide where it was makes it measure again. */
let resizeObserver: ResizeObserver | null = null;

function restartSlides(band: Element) {
    for (const sibling of band.parentElement?.querySelectorAll(`:scope > ${SHIMMER_COPY}`) ?? []) {
        for (const slide of sibling.getAnimations({ subtree: true })) {
            const time = slide.currentTime;
            slide.cancel();
            slide.currentTime = time;
            slide.play();
        }
    }
}

function watchSize(band: Element): () => void {
    resizeObserver ??= new ResizeObserver((entries) => {
        for (const entry of entries) restartSlides(entry.target);
    });
    resizeObserver.observe(band);
    return () => resizeObserver?.unobserve(band);
}

/*
 * The sweep across a live label, for a host with the `chat-shimmer` class. A
 * dim and a bright copy of the text sit behind opposite masks that slide on
 * the compositor, so a running agent repaints nothing per frame. The host's
 * own text only holds the layout and the selection.
 */
export function Shimmer({ children }: { children: ReactNode }) {
    const band = useRef<HTMLSpanElement>(null);
    useLayoutEffect(() => {
        if (!band.current) return;
        return watchSize(band.current);
    }, []);
    return (
        <>
            {children}
            <span ref={band} className="chat-shimmer-band dim" aria-hidden="true" inert>
                <span className="chat-shimmer-copy">
                    <span className="chat-shimmer-text">{children}</span>
                </span>
            </span>
            <span className="chat-shimmer-band lit" aria-hidden="true" inert>
                <span className="chat-shimmer-copy">
                    <span className="chat-shimmer-text">{children}</span>
                </span>
            </span>
        </>
    );
}
