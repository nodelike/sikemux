import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ResizeHandle } from "./ResizeHandle";

let frames: FrameRequestCallback[] = [];

beforeEach(() => {
    frames = [];
    vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => frames.push(callback));
    vi.spyOn(window, "cancelAnimationFrame").mockImplementation(() => {});
    HTMLElement.prototype.setPointerCapture ??= () => {};
});
afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

const runFrames = () => {
    const pending = frames;
    frames = [];
    for (const frame of pending) frame(0);
};

function setup(axis: "x" | "y", grows: 1 | -1, size: number) {
    const target = createRef<HTMLDivElement>();
    const onResize = vi.fn();
    render(
        <>
            <div ref={target} />
            <ResizeHandle
                targetRef={target}
                axis={axis}
                grows={grows}
                min={100}
                max={() => 400}
                size={size}
                label="Resize"
                className="split"
                onResize={onResize}
            />
        </>,
    );
    Object.defineProperty(target.current!, axis === "x" ? "offsetWidth" : "offsetHeight", { value: size });
    return { target: target.current!, handle: screen.getByRole("separator", { name: "Resize" }), onResize };
}

const pointer = (type: "pointerDown" | "pointerMove" | "pointerUp", el: Element, init: { clientX?: number; clientY?: number; button?: number }) =>
    fireEvent[type](el, { pointerId: 1, button: 0, ...init });

it("sizes the box live while dragged and saves the size on release, within its limits", () => {
    const { target, handle, onResize } = setup("x", 1, 200);
    pointer("pointerDown", handle, { clientX: 500 });
    pointer("pointerMove", handle, { clientX: 550 });
    pointer("pointerMove", handle, { clientX: 560 });
    expect(frames).toHaveLength(1);
    runFrames();
    expect(target.style.width).toBe("260px");

    pointer("pointerMove", handle, { clientX: 900 });
    pointer("pointerUp", handle, {});
    expect(onResize).toHaveBeenCalledWith(400);

    pointer("pointerMove", handle, { clientX: 100 });
    expect(frames).toHaveLength(1);
    expect(onResize).toHaveBeenCalledOnce();
});

it("grows a box above it when dragged up, and saves nothing for a drag that goes nowhere", () => {
    const { target, handle, onResize } = setup("y", -1, 150);
    pointer("pointerDown", handle, { clientY: 300 });
    pointer("pointerMove", handle, { clientY: 250 });
    runFrames();
    expect(target.style.flex).toBe("0 0 200px");
    expect(target.style.minHeight).toBe("100px");
    pointer("pointerMove", handle, { clientY: 300 });
    pointer("pointerUp", handle, {});
    expect(onResize).not.toHaveBeenCalled();
});

it("ignores a drag with any button but the primary one", () => {
    const { handle, onResize } = setup("x", 1, 200);
    pointer("pointerDown", handle, { clientX: 500, button: 2 });
    pointer("pointerMove", handle, { clientX: 600 });
    pointer("pointerUp", handle, {});
    expect(frames).toHaveLength(0);
    expect(onResize).not.toHaveBeenCalled();
});

it("steps with the arrows that point the way the box grows, and resets on double-click", () => {
    const { target, handle, onResize } = setup("y", 1, 200);
    fireEvent.keyDown(handle, { key: "ArrowDown" });
    expect(onResize).toHaveBeenLastCalledWith(216);
    fireEvent.keyDown(handle, { key: "ArrowUp", shiftKey: true });
    expect(onResize).toHaveBeenLastCalledWith(136);
    fireEvent.keyDown(handle, { key: "ArrowLeft" });
    expect(onResize).toHaveBeenCalledTimes(2);

    target.style.flex = "0 0 200px";
    fireEvent.doubleClick(handle);
    expect(target.style.flex).toBe("");
    expect(onResize).toHaveBeenLastCalledWith(null);
});
