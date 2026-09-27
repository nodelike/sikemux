import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { contentBox, leavingRef } from "./motion";

let finish: () => void = () => {};

function stubAnimate() {
    const animate = vi.fn(() => {
        let resolve: () => void = () => {};
        const finished = new Promise<void>((r) => (resolve = r));
        finish = resolve;
        return { finished } as unknown as Animation;
    });
    Object.defineProperty(HTMLElement.prototype, "animate", { value: animate, configurable: true, writable: true });
    return animate;
}

function stubReducedMotion(reduced: boolean) {
    vi.stubGlobal("matchMedia", (query: string) => ({ matches: reduced && query.includes("reduce") }) as MediaQueryList);
}

const leave = leavingRef<HTMLElement>((el) => el.animate([{ opacity: 1 }, { opacity: 0 }], { duration: 100 }));

function List({ initial }: { initial: string[] }) {
    const [items, setItems] = useState(initial);
    remove = (id) => setItems((list) => list.filter((item) => item !== id));
    return (
        <ul>
            {items.map((item) => (
                <li key={item} id={`item-${item}`} ref={leave}>
                    {item}
                </li>
            ))}
        </ul>
    );
}
let remove: (id: string) => void = () => {};

describe("leavingRef", () => {
    let host: HTMLDivElement;
    beforeEach(() => {
        host = document.createElement("div");
        document.body.append(host);
    });
    afterEach(() => {
        host.remove();
        vi.unstubAllGlobals();
        delete (HTMLElement.prototype as { animate?: unknown }).animate;
    });

    it("keeps a removed item in its place, inert, until its animation ends", async () => {
        stubAnimate();
        stubReducedMotion(false);
        const root = createRoot(host);
        await act(async () => root.render(<List initial={["a", "b", "c"]} />));
        await act(async () => remove("b"));
        const texts = [...host.querySelectorAll("li")].map((li) => li.textContent);
        expect(texts).toEqual(["a", "b", "c"]);
        const leaving = host.querySelector("li.is-leaving") as HTMLElement;
        expect(leaving.textContent).toBe("b");
        expect(leaving.inert).toBe(true);
        expect(leaving.getAttribute("aria-hidden")).toBe("true");
        expect(leaving.id).toBe("");
        await act(async () => finish());
        expect([...host.querySelectorAll("li")].map((li) => li.textContent)).toEqual(["a", "c"]);
        root.unmount();
    });

    it("removes at once when motion is reduced", async () => {
        stubAnimate();
        stubReducedMotion(true);
        const root = createRoot(host);
        await act(async () => root.render(<List initial={["a", "b"]} />));
        await act(async () => remove("a"));
        expect([...host.querySelectorAll("li")].map((li) => li.textContent)).toEqual(["b"]);
        root.unmount();
    });

    it("leaves nothing behind when the whole list goes", async () => {
        stubAnimate();
        stubReducedMotion(false);
        const root = createRoot(host);
        await act(async () => root.render(<List initial={["a", "b"]} />));
        await act(async () => root.unmount());
        expect(host.querySelectorAll("li")).toHaveLength(0);
    });
});

describe("contentBox", () => {
    it("measures from the container's scrolled content", () => {
        const container = document.createElement("div");
        Object.defineProperty(container, "scrollLeft", { value: 40 });
        container.getBoundingClientRect = () => ({ left: 100, top: 10, width: 300, height: 30 }) as DOMRect;
        expect(contentBox({ left: 150, top: 12, width: 80, height: 28 }, container)).toEqual({ left: 90, top: 2, width: 80, height: 28 });
    });
});
