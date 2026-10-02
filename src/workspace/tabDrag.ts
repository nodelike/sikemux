export type TabPlacement = "before" | "after";

export interface TabDrop {
    targetId: string;
    placement: TabPlacement;
}

/** A tab's horizontal extent on screen, in strip order. */
export interface TabBox {
    id: string;
    left: number;
    right: number;
}

/** Travel before a press becomes a drag, so a click still just selects. */
export const TAB_DRAG_THRESHOLD = 5;

/** How long tabs take to slide aside, and the dragged one to settle. */
export const TAB_SLIDE_MS = 160;

const centre = (box: TabBox) => (box.left + box.right) / 2;

/**
 * The slot the dragged tab reaches after moving `dx`: it passes a neighbour
 * once its leading edge (the right one going right, the left one going left)
 * crosses that neighbour's middle. Using the edge rather than the centre is
 * what lets it pass the outermost tab even though it cannot leave the strip,
 * and a wide tab pass a narrow one.
 */
export function slotAt(boxes: readonly TabBox[], from: number, dx: number): number {
    const held = boxes[from];
    let slot = from;
    if (dx > 0) for (let i = from + 1; i < boxes.length && held.right + dx > centre(boxes[i]); i += 1) slot = i;
    if (dx < 0) for (let i = from - 1; i >= 0 && held.left + dx < centre(boxes[i]); i -= 1) slot = i;
    return slot;
}

/** The drop that puts the tab from `from` into `slot`, or null when it stays put. */
export function dropForSlot(boxes: readonly TabBox[], from: number, slot: number): TabDrop | null {
    if (slot === from || !boxes[slot]) return null;
    return { targetId: boxes[slot].id, placement: slot > from ? "after" : "before" };
}

/**
 * The nearest slot at or short of `slot` that the owner allows. A tab dragged
 * into ground it cannot occupy stops at the last place it could go, so the
 * gap on screen is always one it will really drop into.
 */
export function allowedSlot(
    boxes: readonly TabBox[],
    from: number,
    slot: number,
    canDrop: (sourceId: string, targetId: string, placement: TabPlacement) => boolean,
): number {
    const step = slot > from ? -1 : 1;
    for (let s = slot; s !== from; s += step) {
        const drop = dropForSlot(boxes, from, s);
        if (drop && canDrop(boxes[from].id, drop.targetId, drop.placement)) return s;
    }
    return from;
}

/** How far the tab at `index` slides to open the gap, while `from` is held over `slot`. */
export function slideFor(index: number, from: number, slot: number, width: number): number {
    if (slot > from && index > from && index <= slot) return -width;
    if (slot < from && index >= slot && index < from) return width;
    return 0;
}

/** How far the dragged tab travels from where it started to sit in `slot`. */
export function settleOffset(boxes: readonly TabBox[], from: number, slot: number): number {
    if (slot > from) return boxes[slot].right - boxes[from].right;
    if (slot < from) return boxes[slot].left - boxes[from].left;
    return 0;
}

/** Keeps the dragged tab inside the strip: no further than the outermost tabs. */
export function clampTravel(boxes: readonly TabBox[], from: number, dx: number): number {
    const first = boxes[0];
    const last = boxes[boxes.length - 1];
    return Math.min(Math.max(dx, first.left - boxes[from].left), last.right - boxes[from].right);
}
