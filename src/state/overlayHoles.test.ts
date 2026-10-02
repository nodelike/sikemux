import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useNativeViewHoles } from "./nativeViews";
import { mergeHoles } from "./overlayHoles";

function place(el: Element, left: number, top: number, width: number, height: number) {
    el.getBoundingClientRect = () => ({ left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON: () => ({}) });
}

function add(parent: Element, html: string): HTMLElement {
    parent.insertAdjacentHTML("beforeend", html);
    return parent.lastElementChild as HTMLElement;
}

async function nextFrame() {
    await act(async () => {
        await new Promise((resolve) => requestAnimationFrame(resolve));
    });
}

let app: HTMLElement;
let sheet: HTMLStyleElement;

beforeEach(() => {
    sheet = add(
        document.head,
        "<style>.menu-scrim { position: fixed; inset: 0; } .menu { background: rgb(20, 20, 20); border-top-left-radius: 10px; }</style>",
    ) as HTMLStyleElement;
    app = add(document.body, '<div id="root"><div data-browser-pane></div></div>');
});

afterEach(() => {
    sheet.remove();
    app.remove();
    document.body.querySelectorAll(":scope > .portal").forEach((el) => el.remove());
});

describe("holes for floating surfaces", () => {
    it("cuts a hole where a portalled menu paints, and fills it again when the menu goes", async () => {
        const holes = renderHook(() => useNativeViewHoles());
        const menu = add(document.body, '<div class="portal menu">Rename</div>');
        place(menu, 40, 60, 180, 120);
        await nextFrame();
        expect(holes.result.current).toEqual([{ x: 40, y: 60, width: 180, height: 120, radius: 10 }]);

        menu.remove();
        await nextFrame();
        expect(holes.result.current).toEqual([]);
        holes.unmount();
    });

    it("finds what CSS pins in place inside the app, and gives way through a see-through scrim to the menu in it", async () => {
        const holes = renderHook(() => useNativeViewHoles());
        const scrim = add(app, '<div class="menu-scrim"><div class="menu">Close</div></div>');
        place(scrim, 0, 0, 1200, 800);
        place(scrim.firstElementChild!, 300, 200, 160, 90);
        await nextFrame();
        expect(holes.result.current).toEqual([{ x: 300, y: 200, width: 160, height: 90, radius: 10 }]);
        holes.unmount();
    });

    it("finds a panel marked as floating over the app", async () => {
        const holes = renderHook(() => useNativeViewHoles());
        const peek = add(app, '<div data-overlay><div class="menu">Agents</div></div>');
        place(peek.firstElementChild!, 900, 8, 280, 700);
        await nextFrame();
        expect(holes.result.current).toEqual([{ x: 900, y: 8, width: 280, height: 700, radius: 10 }]);
        holes.unmount();
    });

    it("stops watching once no page reads the holes", async () => {
        const observe = vi.spyOn(MutationObserver.prototype, "disconnect");
        const holes = renderHook(() => useNativeViewHoles());
        holes.unmount();
        expect(observe).toHaveBeenCalled();
        observe.mockRestore();
    });
});

describe("merging holes", () => {
    it("joins holes that cross into the box around both, since crossing holes would paint the page again", () => {
        const tooltip = { x: 100, y: 100, width: 80, height: 20, radius: 6 };
        const panel = { x: 150, y: 0, width: 300, height: 600, radius: 12 };
        const toast = { x: 600, y: 700, width: 200, height: 40, radius: 13 };
        expect(mergeHoles([tooltip, toast, panel])).toEqual([{ x: 100, y: 0, width: 350, height: 600, radius: 6 }, toast]);
    });
});
