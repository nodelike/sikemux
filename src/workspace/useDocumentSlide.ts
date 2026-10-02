import { useLayoutEffect, useRef } from "react";
import type { RefObject } from "react";
import { prefersReducedMotion } from "../lib/motion";
import { PAN_MS } from "./useWindowPan";

/** Falls back to this when no `transitionend` arrives, so a slide can never get stuck. */
const SETTLE_GUARD_MS = PAN_MS + 120;

/**
 * The element inside a pane that holds the document and nothing around it: the
 * file tree, the collection tree and the bars beside them stay where they are.
 */
const DOCUMENT_HOST = ".ed-host, [data-document-host]";

/** Which side the document arriving travels in from: a later document in the list comes from the right. */
export function arrivesFrom(order: readonly string[], leaving: string, arriving: string): -1 | 0 | 1 {
    const was = order.indexOf(leaving);
    const now = order.indexOf(arriving);
    if (was < 0 || now < 0 || was === now) return 0;
    return now > was ? 1 : -1;
}

/** A scrolled element of the document leaving, and how far it was scrolled. */
type Scrolled = readonly [HTMLElement, number, number];

interface Snapshot {
    readonly host: HTMLElement;
    readonly clone: HTMLElement;
    readonly scrolled: readonly Scrolled[];
    readonly from: -1 | 1;
}

interface Slide {
    readonly host: HTMLElement;
    readonly clone: HTMLElement;
    readonly parent: HTMLElement;
    readonly guard: number;
    readonly onEnd: (event: TransitionEvent) => void;
}

function documentHost(layer: HTMLElement | null, paneId: string): HTMLElement | null {
    const pane = layer?.querySelector(`[data-pane-id="${paneId}"]`);
    return pane?.querySelector<HTMLElement>(DOCUMENT_HOST) ?? null;
}

/**
 * A still copy of the document on screen, taken while it is still the one on
 * screen. `cloneNode` carries no scroll position, so every scrolled element is
 * noted here and put back once the copy is in the document and can hold one.
 */
function snapshot(host: HTMLElement, from: -1 | 1): Snapshot {
    const clone = host.cloneNode(true) as HTMLElement;
    const live = [host, ...host.querySelectorAll<HTMLElement>("*")];
    const copies = [clone, ...clone.querySelectorAll<HTMLElement>("*")];
    const scrolled: Scrolled[] = [];
    for (let index = 0; index < live.length; index += 1) {
        const source = live[index];
        if (source.scrollTop || source.scrollLeft) scrolled.push([copies[index], source.scrollTop, source.scrollLeft]);
    }
    // Two of these elements in the document would be two answers to
    // `getElementById`, and the copy is the one nothing should ever reach.
    clone.removeAttribute("id");
    for (const node of clone.querySelectorAll("[id]")) node.removeAttribute("id");
    clone.setAttribute("aria-hidden", "true");
    clone.setAttribute("inert", "");
    clone.classList.add("doc-snapshot");
    clone.classList.remove("doc-slide-in");
    // A copy taken mid-slide would otherwise inherit that slide's own travel.
    for (const property of ["transform", "transition", "will-change"]) clone.style.removeProperty(property);
    return { host, clone, scrolled, from };
}

function stop(slide: Slide): void {
    window.clearTimeout(slide.guard);
    slide.host.removeEventListener("transitionend", slide.onEnd);
    slide.clone.remove();
    slide.host.classList.remove("doc-slide-in");
    slide.host.style.removeProperty("transform");
    slide.parent.classList.remove("doc-sliding", "doc-travelling");
}

const travel = (screens: number) => `translate3d(${screens * 100}%, 0, 0)`;

/**
 * Slides one document of a live window out and the next one in, without a second
 * editor behind it.
 *
 * What leaves is a copy taken before the swap paints: inert, unreachable DOM
 * inside the same screen, so the track's painted layers and its budget are
 * untouched. The live content does the arriving, which is why nothing has to be
 * mounted to show the document being left.
 */
export function useDocumentSlide(
    layerRef: RefObject<HTMLElement | null>,
    paneId: string | null,
    activeDoc: string | null,
    order: readonly string[],
): void {
    const previous = useRef({ paneId, activeDoc });
    const pending = useRef<Snapshot | null>(null);
    const slide = useRef<Slide | null>(null);
    const finish = useRef(() => {
        if (slide.current) stop(slide.current);
        slide.current = null;
    });

    if (previous.current.paneId !== paneId || previous.current.activeDoc !== activeDoc) {
        const was = previous.current;
        previous.current = { paneId, activeDoc };
        // Only a document changing under a pane that stays live slides. A switch
        // between windows is the track's travel, and a pane that was not live has
        // nothing on screen to take a copy of.
        const from = was.paneId === paneId && paneId && was.activeDoc && activeDoc ? arrivesFrom(order, was.activeDoc, activeDoc) : 0;
        // The DOM still holds the document being left: React has not committed the
        // swap yet, which is the only moment a copy of it can be taken.
        if (from !== 0 && !prefersReducedMotion()) {
            const host = documentHost(layerRef.current, paneId!);
            if (host) pending.current = snapshot(host, from);
        }
    }

    useLayoutEffect(() => {
        const next = pending.current;
        pending.current = null;
        if (!next) return;
        const parent = next.host.parentElement;
        if (!parent) return;
        // One copy at a time: a second switch mid-slide drops the one travelling and
        // starts again from the document it was bringing in, so every link of a
        // chain is one screen.
        finish.current();

        const { host, clone, from } = next;
        parent.classList.add("doc-sliding");
        clone.style.left = `${host.offsetLeft}px`;
        clone.style.top = `${host.offsetTop}px`;
        clone.style.width = `${host.offsetWidth}px`;
        clone.style.height = `${host.offsetHeight}px`;
        host.classList.add("doc-slide-in");
        parent.appendChild(clone);
        for (const [node, top, left] of next.scrolled) {
            node.scrollTop = top;
            node.scrollLeft = left;
        }

        clone.style.transform = travel(0);
        host.style.transform = travel(from);
        // Reading layout pins those as the values the travel starts from.
        host.getBoundingClientRect();
        parent.classList.add("doc-travelling");
        clone.style.transform = travel(-from);
        host.style.transform = travel(0);

        const onEnd = (event: TransitionEvent) => {
            if (event.target === host && event.propertyName === "transform") finish.current();
        };
        host.addEventListener("transitionend", onEnd);
        slide.current = { host, clone, parent, onEnd, guard: window.setTimeout(() => finish.current(), SETTLE_GUARD_MS) };
    });

    useLayoutEffect(() => () => finish.current(), []);
}
