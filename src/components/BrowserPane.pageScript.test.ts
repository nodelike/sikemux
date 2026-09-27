import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeAll, beforeEach, describe, expect, it } from "vitest";

/* The script an agent's browser tools run inside a tab. jsdom has no layout,
   so each element's box comes from its data-rect="left,top,width,height" and
   the topmost box under a point is the last one in the document. */
const PAGE_SCRIPT = readFileSync(resolve(__dirname, "../../src-tauri/src/browser/page.js"), "utf8");

interface Changes {
    elements?: string;
    removed?: number[];
    textAdded?: string;
    textRemoved?: string;
}

interface PageApi {
    state(mode?: string, fullText?: boolean): { elements?: string; text?: string; textLength?: number; changes?: Changes | "none" };
    find(query: string, role?: string | null): { matches: number; elements: string; note?: string };
    locate(text: string, role?: string | null): number;
    point(index: number, expectLabel?: string | null): { index: number; label: string; covered: string | null };
    showMarks(visible: boolean): { marked?: number[] };
}

const page = () => (window as unknown as { __sikemux: PageApi }).__sikemux;

const boxOf = (element: Element) => {
    const [left, top, width, height] = (element.getAttribute("data-rect") ?? "0,0,0,0").split(",").map(Number);
    return { left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON: () => ({}) } as DOMRect;
};

const numberOf = (elements: string | undefined, label: string) => {
    const line = (elements ?? "").split("\n").find((item) => item.endsWith(`> ${label}`));
    return Number(/^\[(\d+)\]/.exec(line ?? "")?.[1]);
};

const load = (html: string) => {
    document.body.innerHTML = html;
    delete (window as unknown as Record<string, unknown>).__sikemux;
    delete (window as unknown as Record<string, unknown>).__sikemuxNumbers;
    delete (window as unknown as Record<string, unknown>).__sikemuxRefs;
    delete (window as unknown as Record<string, unknown>).__sikemuxLast;
    (0, eval)(PAGE_SCRIPT);
};

beforeAll(() => {
    Object.defineProperty(HTMLElement.prototype, "innerText", {
        configurable: true,
        get(this: HTMLElement) {
            return this.textContent ?? "";
        },
    });
    HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
        return boxOf(this);
    };
    Element.prototype.scrollIntoView = () => {};
    document.elementFromPoint = (x: number, y: number) => {
        const under = [...document.querySelectorAll("[data-rect]")].filter((element) => {
            if (element.closest("[inert]")) return false;
            const box = boxOf(element);
            return x >= box.left && x < box.right && y >= box.top && y < box.bottom;
        });
        return under.at(-1) ?? document.body;
    };
});

beforeEach(() => load(""));

describe("element numbers", () => {
    it("stay with their element when the list around it re-renders", () => {
        load(`<button data-rect="0,0,80,20">Cardiologist</button>\n<button data-rect="0,30,80,20">Neurologist</button>`);
        const first = page().state("full");
        const neurologist = numberOf(first.elements, "Neurologist");

        const chip = document.createElement("button");
        chip.textContent = "Dermatologist";
        chip.setAttribute("data-rect", "0,60,80,20");
        document.body.prepend(chip);
        const second = page().state("full");

        expect(numberOf(second.elements, "Neurologist")).toBe(neurologist);
        expect(page().point(neurologist).label).toBe("Neurologist");
    });

    it("refuse a number whose element has left the page", () => {
        load(`<button data-rect="0,0,80,20">Save</button>`);
        const save = numberOf(page().state("full").elements, "Save");
        document.body.innerHTML = `<button data-rect="0,0,80,20">Save</button>`;

        expect(() => page().point(save)).toThrow(/no element/);
    });

    it("refuse a click whose element no longer carries the expected label", () => {
        load(`<button data-rect="0,0,80,20">Neurologist</button>`);
        const index = numberOf(page().state("full").elements, "Neurologist");
        document.querySelector("button")!.textContent = "Nephrologist";

        expect(() => page().point(index, "Neurologist")).toThrow(/not "Neurologist"; nothing was clicked/);
        expect(page().point(index, "nephro").label).toBe("Nephrologist");
    });
});

describe("page state", () => {
    it("reports only what changed since the last read", () => {
        load(`<p data-rect="0,0,200,20">Inbox</p>\n<button data-rect="0,30,80,20">Refresh</button>\n<button data-rect="0,60,80,20">Archive</button>`);
        const first = page().state("changes");
        expect(first.elements).toContain("Refresh");

        document.querySelectorAll("button")[1].remove();
        const toast = document.createElement("button");
        toast.textContent = "Undo";
        toast.setAttribute("data-rect", "0,90,80,20");
        document.body.append("\n", toast);
        const next = page().state("changes").changes as Changes;

        expect(next.elements).toMatch(/^\[\d+\] <button[^>]*> Undo$/);
        expect(next.removed).toEqual([numberOf(first.elements, "Archive")]);
        expect(next.textAdded).toBe("Undo");
        expect(next.textRemoved).toBe("Archive");
        expect(page().state("changes").changes).toBe("none");
    });

    it("caps the text unless the full text is asked for", () => {
        load(`<p data-rect="0,0,200,20">${"word ".repeat(1000)}</p>`);

        const capped = page().state("full");
        expect(capped.text).toHaveLength(2001);
        expect(capped.textLength).toBe(4999);
        expect(page().state("full", true).text).toHaveLength(4999);
    });
});

describe("finding by label", () => {
    beforeEach(() =>
        load(
            `<a href="/save" data-rect="0,0,80,20">Save draft</a>\n<button data-rect="0,30,80,20">Save</button>\n<button data-rect="0,60,80,20" aria-label="Close dialog">×</button>\n<div role="tab" data-rect="0,90,80,20">Save</div>`,
        ),
    );

    it("clicks the one element named that way, by text or accessible name", () => {
        const close = page().locate("close dialog");
        expect(page().point(close).label).toBe("Close dialog");
        expect(page().point(page().locate("Save", "button")).label).toBe("Save");
    });

    it("refuses an ambiguous name and lists the candidates", () => {
        expect(() => page().locate("Save")).toThrow(/2 elements match "Save"; nothing was clicked[\s\S]*<button[\s\S]*<div>/);
        expect(() => page().locate("Delete")).toThrow(/nothing is labelled "Delete"/);
    });

    it("finds partial matches with their numbers", () => {
        const found = page().find("save");
        expect(found.matches).toBe(2);
        expect(page().find("sav").matches).toBe(3);
        expect(page().find("sav", "link").elements).toMatch(/<a> Save draft/);
    });
});

describe("annotated screenshots", () => {
    it("box only what a person can see", () => {
        load(
            [
                `<main data-rect="0,0,1024,768" tabindex="0">`,
                `<button data-rect="10,10,80,20">Visible</button>`,
                `<button data-rect="10,40,80,20" inert>Inert</button>`,
                `<div aria-hidden="true"><button data-rect="10,70,80,20">Hidden from readers</button></div>`,
                `<button data-rect="-300,100,80,20">Off canvas</button>`,
                `<button data-rect="10,130,80,20">Covered</button>`,
                `</main>`,
                `<div data-rect="0,120,200,60">sticky header</div>`,
            ].join("\n"),
        );
        const elements = page().state("full").elements;
        const marked = page().showMarks(true).marked;

        expect(marked).toEqual([numberOf(elements, "Visible")]);
    });

    it("mark only what sits inside an open modal", () => {
        load(
            [
                `<button data-rect="10,10,80,20">Behind</button>`,
                `<div role="dialog" aria-modal="true" data-rect="200,200,300,200"><button data-rect="220,220,80,20">Confirm</button></div>`,
            ].join("\n"),
        );
        const elements = page().state("full").elements;

        expect(page().showMarks(true).marked).toEqual([numberOf(elements, "Confirm")]);
    });

    it("keep a box larger than most of the view only when it is alone", () => {
        load(`<div tabindex="0" data-rect="0,0,1024,768">Card</div>`);
        page().state("full");
        expect(page().showMarks(true).marked).toHaveLength(1);

        load(`<div tabindex="0" data-rect="0,0,1024,768">Card\n<button data-rect="10,10,80,20">Open</button></div>`);
        const elements = page().state("full").elements;
        expect(page().showMarks(true).marked).toEqual([numberOf(elements, "Open")]);
    });
});
